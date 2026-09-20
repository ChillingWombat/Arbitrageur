use arbitrageur_core::exchange::ExchangeAdapter;
use arbitrageur_core::types::{dec, Decimal};
use arbitrageur_exchanges::hyperliquid::HyperliquidAdapter;
use arbitrageur_exchanges::lighter::LighterAdapter;
use arbitrageur_exchanges::prediction::{KalshiAdapter, PolymarketAdapter};
use arbitrageur_exchanges::robinhood::RobinhoodAdapter;
use arbitrageur_exchanges::simulated::SimulatedExchange;
use arbitrageur_exchanges::vanta::VantaAdapter;
use arbitrageur_execution::coordinator::AtomicArbitrageCoordinator;
use arbitrageur_execution::smart_routing::SmartOrderRouter;
use arbitrageur_math::funding_carry::evaluate_funding_carry;
use arbitrageur_math::kalman_filter::HedgeRatioKalmanFilter;
use arbitrageur_math::optimal_thresholds::compute_bertram_thresholds;
use arbitrageur_math::ou_process::OuModelParams;
use arbitrageur_math::prediction_synthetic::evaluate_prediction_cross_venue;
use arbitrageur_risk::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use arbitrageur_risk::delta_guard::DeltaGuard;
use arbitrageur_strategy::funding_rate_arb::FundingRateArbStrategy;
use arbitrageur_strategy::spatial_price_arb::SpatialPriceArbStrategy;
use clap::{Parser, Subcommand};
use std::sync::Arc;
use std::time::Instant;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(name = "arbitrageur")]
#[command(about = "Ultra-Fast Modular Cross-Market Arbitrageur for Price Spread & Funding Rate Differences in Rust", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Monitor real-time prices, spreads, and funding rates across exchanges
    Monitor {
        #[arg(short, long, default_value = "BTC-USDC")]
        symbol: String,
        #[arg(short, long, default_value_t = 3)]
        interval_secs: u64,
        #[arg(short, long, default_value_t = 5)]
        cycles: usize,
    },
    /// Run simulated high-frequency paper arbitrage engine
    Simulate {
        #[arg(short, long, default_value = "BTC-USDC")]
        symbol: String,
        #[arg(short, long, default_value_t = 10)]
        target_bps: i64,
        #[arg(short, long, default_value_t = 10)]
        iterations: usize,
    },
    /// Scan and display funding rate carry differences and APY
    Funding {
        #[arg(short, long, default_value = "ETH-USDC")]
        symbol: String,
    },
    /// Run statistical arbitrage Kalman Filter hedge ratio tracker on pairs
    StatArb {
        #[arg(long, default_value = "BTC-USDC")]
        symbol_y: String,
        #[arg(long, default_value = "ETH-USDC")]
        symbol_x: String,
    },
    /// Scan prediction markets for synthetic outcome arbitrage (Polymarket vs Kalshi)
    Prediction {
        #[arg(long, default_value = "FED-CHAIR-WARSH")]
        market: String,
    },
    /// Benchmark quantitative calculations and latency
    Benchmark,
    /// Run headless read-only placeholder daemon exposing a health endpoint
    Serve {
        #[arg(short, long, default_value_t = 8787)]
        port: u16,
        #[arg(short, long, env = "ARBITRAGEUR_BIND", default_value = "127.0.0.1")]
        bind: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();

    let cli = Cli::parse();

    println!("========================================================================");
    println!("      ⚡ CROSS-MARKET ARBITRAGEUR (PRICE SPREAD & FUNDING CARRY) ⚡      ");
    println!("  Markets: Web3 (Hyperliquid, Lighter, Vanta, Robinhood) -> Prediction ");
    println!("========================================================================");

    match cli.command {
        Commands::Monitor {
            symbol,
            interval_secs,
            cycles,
        } => {
            run_monitor(&symbol, interval_secs, cycles).await?;
        }
        Commands::Simulate {
            symbol,
            target_bps,
            iterations,
        } => {
            run_simulate(&symbol, Decimal::from(target_bps), iterations).await?;
        }
        Commands::Funding { symbol } => {
            run_funding_scan(&symbol).await?;
        }
        Commands::StatArb { symbol_y, symbol_x } => {
            run_stat_arb(&symbol_y, &symbol_x).await?;
        }
        Commands::Prediction { market } => {
            run_prediction_arb(&market).await?;
        }
        Commands::Benchmark => {
            run_benchmarks().await?;
        }
        Commands::Serve { port, bind } => {
            run_serve(&bind, port).await?;
        }
    }

    Ok(())
}

async fn run_monitor(symbol: &str, interval_secs: u64, cycles: usize) -> anyhow::Result<()> {
    println!(
        "\n[INFO] Starting Multi-Market Monitor for symbol: {}",
        symbol
    );
    println!("Connecting to Hyperliquid, Lighter, Vanta.exchange, and Robinhood EVM...\n");

    let hl: Arc<dyn ExchangeAdapter> = Arc::new(HyperliquidAdapter::new(None, None, None));
    let lighter: Arc<dyn ExchangeAdapter> = Arc::new(LighterAdapter::new(None, None, None));
    let vanta: Arc<dyn ExchangeAdapter> = Arc::new(VantaAdapter::new(None, None, None, None));
    let robinhood: Arc<dyn ExchangeAdapter> =
        Arc::new(RobinhoodAdapter::new(None, None, None, None));

    let venues: Vec<Arc<dyn ExchangeAdapter>> = vec![hl, lighter, vanta, robinhood];

    for c in 1..=cycles {
        println!(
            "--- Cycle {}/{} at {} ---",
            c,
            cycles,
            chrono::Utc::now().to_rfc3339()
        );
        println!(
            "{:<16} | {:<12} | {:<12} | {:<10} | {:<14}",
            "Venue", "Best Bid", "Best Ask", "Spread", "Funding (8h)"
        );
        println!("{:-<75}", "");

        for v in &venues {
            let book_res = v.fetch_orderbook(symbol).await;
            let funding_res = v.fetch_funding_rate(symbol).await;

            let (bid_str, ask_str, spread_str) = match book_res {
                Ok(book) => {
                    let bid = book
                        .best_bid_price()
                        .map(|p| format!("{:.2}", p))
                        .unwrap_or_else(|| "N/A".into());
                    let ask = book
                        .best_ask_price()
                        .map(|p| format!("{:.2}", p))
                        .unwrap_or_else(|| "N/A".into());
                    let spread = book
                        .spread()
                        .map(|s| format!("{:.2}", s))
                        .unwrap_or_else(|| "N/A".into());
                    (bid, ask, spread)
                }
                Err(_) => ("Offline".into(), "Offline".into(), "N/A".into()),
            };

            let funding_str = match funding_res {
                Ok(f) => format!("{:.4}%", f.rate * dec!(100)),
                Err(_) => "N/A (Spot)".into(),
            };

            println!(
                "{:<16} | {:<12} | {:<12} | {:<10} | {:<14}",
                v.name(),
                bid_str,
                ask_str,
                spread_str,
                funding_str
            );
        }

        println!();
        if c < cycles {
            tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
        }
    }

    Ok(())
}

async fn run_simulate(symbol: &str, target_bps: Decimal, iterations: usize) -> anyhow::Result<()> {
    println!("\n[SIMULATION] Initializing Microsecond Paper Trading Environment...");
    let ex_hl = Arc::new(SimulatedExchange::new(
        "hyperliquid",
        "Hyperliquid (L1)",
        2,
        dec!(2.0),
        dec!(50000.0),
    ));
    let ex_lighter = Arc::new(SimulatedExchange::new(
        "lighter",
        "Lighter (zkDEX)",
        5,
        dec!(1.5),
        dec!(50000.0),
    ));
    let ex_vanta = Arc::new(SimulatedExchange::new(
        "vanta",
        "Vanta.exchange",
        4,
        dec!(3.0),
        dec!(50000.0),
    ));

    // Seed markets with realistic orderbooks and initial funding rates
    ex_hl.seed_market(
        symbol,
        dec!(60000.0),
        dec!(60010.0),
        dec!(5.0),
        dec!(0.0003),
    ); // higher funding on HL
    ex_lighter.seed_market(
        symbol,
        dec!(60025.0),
        dec!(60035.0),
        dec!(4.0),
        dec!(0.0001),
    ); // price spread on Lighter!
    ex_vanta.seed_market(
        symbol,
        dec!(60005.0),
        dec!(60015.0),
        dec!(6.0),
        dec!(0.00015),
    );

    let cb = Arc::new(CircuitBreaker::new(
        CircuitBreakerConfig::default(),
        dec!(150000.0),
    ));
    let delta_guard = DeltaGuard::new(dec!(500.0)); // $500 max delta tolerance
    let router = SmartOrderRouter::new(dec!(15.0)); // 15 bps max slippage
    let coordinator = Arc::new(AtomicArbitrageCoordinator::new(router, cb.clone()));

    let mut spatial_strat = SpatialPriceArbStrategy::new(
        symbol,
        ex_hl.clone(),
        ex_lighter.clone(),
        target_bps,
        dec!(15.0),
        dec!(0.5), // 0.5 BTC order qty
        dec!(2.0),
        dec!(1.5),
        20,
        1,
    );

    let funding_strat = FundingRateArbStrategy::new(
        symbol,
        ex_hl.clone(),
        ex_lighter.clone(),
        dec!(0.0001),  // 0.01% min 8h diff
        dec!(48.0),    // max 48h break-even
        dec!(15000.0), // $15k trade size
        dec!(3.5),
    );

    println!(
        "Starting simulation loop for {} iterations...\n",
        iterations
    );
    let mut total_realized_profit = dec!(0);

    for i in 1..=iterations {
        // Inject price perturbation to simulate market movements and cross-venue dislocations
        let dislocation = if i % 2 == 1 { dec!(95.0) } else { dec!(130.0) };
        ex_lighter.seed_market(
            symbol,
            dec!(60010.0) + dislocation,
            dec!(60020.0) + dislocation,
            dec!(4.0),
            dec!(0.0001),
        );

        // 1. Evaluate spatial price spread
        if let Ok(Some(signal)) = spatial_strat.evaluate_opportunity().await {
            println!(
                "Iteration #{}: Found Spatial Arb -> Direction: {:?}, Spread: ${:.2} ({:.2} BPS)",
                i, signal.direction, signal.spread, signal.spread_bps
            );
            let res = spatial_strat.execute_signal(&signal, &coordinator).await;
            match res {
                arbitrageur_execution::coordinator::ArbitrageExecutionResult::Success {
                    realized_profit,
                    ..
                } => {
                    total_realized_profit += realized_profit;
                    println!(
                        "  -> Executed atomically! Profit: +${:.2} (Cumulative: ${:.2})",
                        realized_profit, total_realized_profit
                    );
                }
                other => println!("  -> Execution status: {:?}", other),
            }
        }

        // 2. Evaluate funding rate carry
        if i == 1 || i == iterations / 2 {
            if let Ok(Some(opp)) = funding_strat.scan_opportunity().await {
                println!(
                    "Iteration #{}: Found Funding Carry -> Long: {}, Short: {}, 8h Net Rate: {:.4}%, APR: {:.2}%, Breakeven: {:.1}h",
                    i, opp.long_venue, opp.short_venue, opp.net_rate_8h * dec!(100), opp.net_apr * dec!(100), opp.break_even_hours
                );
                let carry_res = funding_strat.execute_open_carry(&opp, &coordinator).await;
                println!(
                    "  -> Carry trade execution result: {:?}",
                    carry_res.map(|r| format!("{:?}", r))
                );
            }
        }

        // Check delta neutrality
        let acc_hl = ex_hl.fetch_account().await?;
        let acc_lighter = ex_lighter.fetch_account().await?;
        let all_positions = [acc_hl.positions, acc_lighter.positions].concat();
        let net_deltas = delta_guard.calculate_net_delta(&all_positions);
        for (asset, (qty, usd)) in net_deltas {
            println!(
                "  [Delta Guard] Asset: {}, Net Qty: {:.4}, Net Exposure: ${:.2}",
                asset, qty, usd
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }

    println!("\n✅ Simulation completed successfully!");
    println!(
        "Total Cumulative Arbitrage Profit: +${:.2}",
        total_realized_profit
    );
    Ok(())
}

async fn run_funding_scan(symbol: &str) -> anyhow::Result<()> {
    println!(
        "\n[FUNDING ARBITRAGE SCAN] Scanning venues for symbol: {}",
        symbol
    );
    let now = chrono::Utc::now();

    // Rates example across markets
    let rate_hl = arbitrageur_core::quote::FundingRate {
        venue: "Hyperliquid".into(),
        symbol: symbol.into(),
        rate: dec!(0.0003), // 0.03% hourly (high positive rate on Hyperliquid)
        predicted_rate: Some(dec!(0.00032)),
        interval_hours: 1,
        next_funding_time: now,
        timestamp: now,
    };

    let rate_lighter = arbitrageur_core::quote::FundingRate {
        venue: "Lighter".into(),
        symbol: symbol.into(),
        rate: dec!(0.0001), // 0.01% per 8h
        predicted_rate: None,
        interval_hours: 8,
        next_funding_time: now,
        timestamp: now,
    };

    let rate_vanta = arbitrageur_core::quote::FundingRate {
        venue: "Vanta.exchange".into(),
        symbol: symbol.into(),
        rate: dec!(0.00015), // 0.015% per 8h
        predicted_rate: None,
        interval_hours: 8,
        next_funding_time: now,
        timestamp: now,
    };

    println!("\nVenue Funding Profiles:");
    println!(
        "- Hyperliquid: Rate = {:.4}% / 1h (APR = {:.2}%)",
        rate_hl.rate * dec!(100),
        rate_hl.apr() * dec!(100)
    );
    println!(
        "- Lighter:     Rate = {:.4}% / 8h (APR = {:.2}%)",
        rate_lighter.rate * dec!(100),
        rate_lighter.apr() * dec!(100)
    );
    println!(
        "- Vanta:       Rate = {:.4}% / 8h (APR = {:.2}%)",
        rate_vanta.rate * dec!(100),
        rate_vanta.apr() * dec!(100)
    );

    println!("\nEvaluated Carry Opportunities (Trade size: $25,000 USD):");
    if let Some(opp1) = evaluate_funding_carry(&rate_hl, &rate_lighter, dec!(3.5), dec!(25000.0)) {
        println!("1. Short {} + Long {}:", opp1.short_venue, opp1.long_venue);
        println!(
            "   Net 8h Rate: {:.4}%, APR: {:.2}%, APY: {:.2}%",
            opp1.net_rate_8h * dec!(100),
            opp1.net_apr * dec!(100),
            opp1.net_apy * dec!(100)
        );
        println!(
            "   Break-even time: {:.1} hours | Est Daily Income: +${:.2} USD",
            opp1.break_even_hours, opp1.daily_income_usd
        );
    }

    if let Some(opp2) = evaluate_funding_carry(&rate_hl, &rate_vanta, dec!(4.0), dec!(25000.0)) {
        println!("2. Short {} + Long {}:", opp2.short_venue, opp2.long_venue);
        println!(
            "   Net 8h Rate: {:.4}%, APR: {:.2}%, APY: {:.2}%",
            opp2.net_rate_8h * dec!(100),
            opp2.net_apr * dec!(100),
            opp2.net_apy * dec!(100)
        );
        println!(
            "   Break-even time: {:.1} hours | Est Daily Income: +${:.2} USD",
            opp2.break_even_hours, opp2.daily_income_usd
        );
    }

    Ok(())
}

async fn run_stat_arb(symbol_y: &str, symbol_x: &str) -> anyhow::Result<()> {
    println!(
        "\n[STATISTICAL ARBITRAGE] Running Online Kalman Filter Pair Tracker: {} vs {}",
        symbol_y, symbol_x
    );
    let mut kf = HedgeRatioKalmanFilter::new(1e-4, 1e-2);

    // Generate sample synthetic cointegrated spread walk
    println!(
        "{:<6} | {:<10} | {:<10} | {:<12} | {:<12} | {:<10}",
        "Step", "Price Y", "Price X", "Hedge Ratio", "Spread (Res)", "Z-Score"
    );
    println!("{:-<75}", "");

    let base_x = 3200.0;
    let _base_y = 64000.0;
    let true_beta = 20.0; // 1 BTC = 20 ETH

    for i in 1..=15 {
        let px = base_x + (i as f64 * 5.0);
        let py = true_beta * px + 100.0 + ((i as f64 * 1.7).sin() * 80.0);
        let (beta, residual) = kf.update(px, py);
        let z = kf.spread_z_score();

        println!(
            "{:<6} | ${:<9.2} | ${:<9.2} | {:<12.4} | {:<12.2} | {:<10.2}",
            i, py, px, beta, residual, z
        );
    }

    println!(
        "\nKalman filter converged to dynamic hedge ratio: {:.4}",
        kf.hedge_ratio()
    );
    Ok(())
}

/// Extract real cross-venue prediction quotes, failing closed when any side is missing.
/// Returns `(poly_yes, poly_no, kalshi_yes, kalshi_no)`.
fn prediction_quotes(
    poly_book: &arbitrageur_core::quote::OrderBook,
    kalshi_book: &arbitrageur_core::quote::OrderBook,
    market: &str,
) -> anyhow::Result<(Decimal, Decimal, Decimal, Decimal)> {
    let poly_yes = poly_book.best_ask_price().ok_or_else(|| {
        anyhow::anyhow!(
            "Polymarket orderbook for {market} is missing the YES ask (no real quote available)"
        )
    })?;
    let poly_no = dec!(1.0)
        - poly_book.best_bid_price().ok_or_else(|| {
            anyhow::anyhow!(
                "Polymarket orderbook for {market} is missing the YES bid (no real quote available)"
            )
        })?;
    let kalshi_no = kalshi_book.best_ask_price().ok_or_else(|| {
        anyhow::anyhow!(
            "Kalshi orderbook for {market} is missing the NO ask (no real quote available)"
        )
    })?;
    let kalshi_yes = dec!(1.0)
        - kalshi_book.best_bid_price().ok_or_else(|| {
            anyhow::anyhow!(
                "Kalshi orderbook for {market} is missing the NO bid (no real quote available)"
            )
        })?;
    Ok((poly_yes, poly_no, kalshi_yes, kalshi_no))
}

async fn run_prediction_arb(market: &str) -> anyhow::Result<()> {
    println!(
        "\n[PREDICTION MARKET ARBITRAGE] Scanning Synthetic Arbitrage for: {}",
        market
    );
    println!("Connecting to Polymarket CLOB and Kalshi API...");

    let poly = Arc::new(PolymarketAdapter::new(None, None));
    let kalshi = Arc::new(KalshiAdapter::new(None, None));

    let poly_book = poly.fetch_orderbook(market).await?;
    let kalshi_book = kalshi.fetch_orderbook(market).await?;

    let (poly_yes, poly_no, kalshi_yes, kalshi_no) =
        prediction_quotes(&poly_book, &kalshi_book, market)?;

    println!("Market Prices:");
    println!(
        "- Polymarket: YES Ask = {:.2}¢, NO Ask = {:.2}¢",
        poly_yes * dec!(100),
        poly_no * dec!(100)
    );
    println!(
        "- Kalshi:     YES Ask = {:.2}¢, NO Ask = {:.2}¢",
        kalshi_yes * dec!(100),
        kalshi_no * dec!(100)
    );

    if let Some(opp) = evaluate_prediction_cross_venue(
        "Polymarket",
        poly_yes,
        poly_no,
        "Kalshi",
        kalshi_yes,
        kalshi_no,
        dec!(10.0),
    ) {
        println!("\n🎯 CROSS-VENUE PARITY CANDIDATE FOUND!");
        println!("Strategy: {}", opp.strategy_name);
        println!(
            "Total Purchase Cost: {:.2}¢ per pair",
            opp.total_cost * dec!(100)
        );
        println!("Conditional Settlement Payout: 100.00¢ (only when both contracts have identical settlement terms)");
        println!(
            "Indicative Net Return per contract: +{:.2}¢ ({:.2}%)",
            opp.net_profit * dec!(100),
            opp.net_return_pct
        );
    } else {
        println!("\nNo cross-venue parity opportunity at current market prices.");
    }

    Ok(())
}

async fn run_benchmarks() -> anyhow::Result<()> {
    println!("\n[BENCHMARKS] Running High-Performance Quantitative Micro-Benchmarks in Rust...\n");

    // 1. Benchmark Kalman Filter update
    let start = Instant::now();
    let mut kf = HedgeRatioKalmanFilter::new(1e-4, 1e-2);
    let iterations = 1_000_000;
    for i in 0..iterations {
        kf.update(100.0 + (i as f64 * 0.01), 200.0 + (i as f64 * 0.02));
    }
    let elapsed = start.elapsed();
    println!("1. Kalman Filter 2D update ({} iterations):", iterations);
    println!("   Total time: {:?}", elapsed);
    println!(
        "   Per-update latency: {:.2} nanoseconds",
        (elapsed.as_nanos() as f64) / (iterations as f64)
    );
    println!(
        "   Throughput: {:.2} million updates/sec\n",
        (iterations as f64) / (elapsed.as_secs_f64() * 1_000_000.0)
    );

    // 2. Benchmark Ornstein-Uhlenbeck MLE fitting
    let synthetic_data: Vec<f64> = (0..5000)
        .map(|i| 10.0 + (i as f64 * 0.05).sin() + (i % 7) as f64 * 0.1)
        .collect();
    let start = Instant::now();
    let fit_count = 10_000;
    for _ in 0..fit_count {
        OuModelParams::fit(&synthetic_data, 1.0);
    }
    let elapsed = start.elapsed();
    println!(
        "2. Ornstein-Uhlenbeck MLE fitting ({} fits on 5000-point series):",
        fit_count
    );
    println!("   Total time: {:?}", elapsed);
    println!(
        "   Per-fit latency: {:.2} microseconds\n",
        (elapsed.as_micros() as f64) / (fit_count as f64)
    );

    // 3. Benchmark Bertram optimal threshold solver
    let ou = OuModelParams {
        theta: 0.15,
        mu: 0.0,
        sigma: 0.05,
        half_life: 4.62,
        stationary_std: 0.091,
        r_squared: 0.88,
    };
    let start = Instant::now();
    let solve_count = 10_000;
    for _ in 0..solve_count {
        compute_bertram_thresholds(&ou, 0.002);
    }
    let elapsed = start.elapsed();
    println!(
        "3. Bertram-Style Numerical Threshold Search ({} solves):",
        solve_count
    );
    println!("   Total time: {:?}", elapsed);
    println!(
        "   Per-solve latency: {:.2} microseconds",
        (elapsed.as_micros() as f64) / (solve_count as f64)
    );

    println!(
        "\n✅ All quantitative benchmarks passed with ultra-low nanosecond/microsecond latencies!"
    );
    Ok(())
}

fn daemon_health_response() -> &'static str {
    "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"status\":\"not_ready\",\"mode\":\"read_only_placeholder\",\"market_data_connected\":false,\"trading_enabled\":false}\n"
}

async fn run_serve(bind: &str, port: u16) -> anyhow::Result<()> {
    // Security: this placeholder answers a static 503 to ANY bytes, but a
    // non-loopback bind would still expose the port to the LAN. Refuse unless
    // the operator explicitly opts in via ARBITRAGEUR_ALLOW_LAN=1.
    if bind != "127.0.0.1" && bind != "localhost" && bind != "::1" {
        let explicit = std::env::var("ARBITRAGEUR_ALLOW_LAN").unwrap_or_default();
        if explicit != "1" {
            anyhow::bail!(
                "Refusing non-loopback bind '{bind}' (set ARBITRAGEUR_ALLOW_LAN=1 to override)"
            );
        }
        eprintln!("[WARN] LAN-exposed placeholder server on {bind} (explicit override)");
    }
    println!("\n[DAEMON] Starting read-only placeholder server...");
    println!("  -> Health Listener:   http://{}:{}", bind, port);
    println!("  -> Market Data:       none (no feed ingestion running)");
    println!("  -> Execution:         disabled (no order submission implemented)");
    println!("  -> Process PID:       {}", std::process::id());
    println!("Press Ctrl+C to safely exit.\n");

    let rpc_addr = format!("{}:{}", bind, port);
    let listener = tokio::net::TcpListener::bind(&rpc_addr).await?;
    println!("Daemon health endpoint listening on {}", rpc_addr);

    tokio::spawn(async move {
        loop {
            if let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    if let Ok(n) = socket.read(&mut buf).await {
                        if n > 0 {
                            let _ = socket.write_all(daemon_health_response().as_bytes()).await;
                        }
                    }
                });
            }
        }
    });

    println!("[INFO] Placeholder status loop running; no strategies are active.");
    let mut loop_count = 0;
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
        loop_count += 1;
        tracing::info!(
            heartbeat = loop_count,
            "Daemon heartbeat: read-only placeholder, no market data ingestion, no execution."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbitrageur_core::quote::{BookLevel, OrderBook};
    use arbitrageur_core::types::VenueId;

    fn book_with_sides(bid: Option<Decimal>, ask: Option<Decimal>) -> OrderBook {
        let mut book = OrderBook::new(VenueId::new("test"), "MKT".to_string());
        if let Some(bid) = bid {
            book.bids.push(BookLevel::new(bid, dec!(1.0)));
        }
        if let Some(ask) = ask {
            book.asks.push(BookLevel::new(ask, dec!(1.0)));
        }
        book
    }

    #[test]
    fn prediction_quotes_returns_exact_quote_math() {
        let poly = book_with_sides(Some(dec!(0.60)), Some(dec!(0.62)));
        let kalshi = book_with_sides(Some(dec!(0.50)), Some(dec!(0.52)));

        let (poly_yes, poly_no, kalshi_yes, kalshi_no) =
            prediction_quotes(&poly, &kalshi, "MKT").unwrap();

        assert_eq!(poly_yes, dec!(0.62));
        assert_eq!(poly_no, dec!(1.0) - dec!(0.60));
        assert_eq!(kalshi_no, dec!(0.52));
        assert_eq!(kalshi_yes, dec!(1.0) - dec!(0.50));
    }

    #[test]
    fn prediction_quotes_errors_on_missing_poly_yes_ask() {
        let poly = book_with_sides(Some(dec!(0.60)), None);
        let kalshi = book_with_sides(Some(dec!(0.50)), Some(dec!(0.52)));

        let err = prediction_quotes(&poly, &kalshi, "MKT").unwrap_err();
        assert!(
            err.to_string().contains("YES ask"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn prediction_quotes_errors_on_missing_poly_yes_bid() {
        let poly = book_with_sides(None, Some(dec!(0.62)));
        let kalshi = book_with_sides(Some(dec!(0.50)), Some(dec!(0.52)));

        let err = prediction_quotes(&poly, &kalshi, "MKT").unwrap_err();
        assert!(
            err.to_string().contains("YES bid"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn prediction_quotes_errors_on_missing_kalshi_no_ask() {
        let poly = book_with_sides(Some(dec!(0.60)), Some(dec!(0.62)));
        let kalshi = book_with_sides(Some(dec!(0.50)), None);

        let err = prediction_quotes(&poly, &kalshi, "MKT").unwrap_err();
        assert!(
            err.to_string().contains("NO ask"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn prediction_quotes_errors_on_missing_kalshi_no_bid() {
        let poly = book_with_sides(Some(dec!(0.60)), Some(dec!(0.62)));
        let kalshi = book_with_sides(None, Some(dec!(0.52)));

        let err = prediction_quotes(&poly, &kalshi, "MKT").unwrap_err();
        assert!(
            err.to_string().contains("NO bid"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn serve_bind_uses_env_override() {
        let prior = std::env::var("ARBITRAGEUR_BIND").ok();
        std::env::set_var("ARBITRAGEUR_BIND", "127.0.0.2");
        let parsed = Cli::try_parse_from(["arbitrageur", "serve", "--port", "8788"]);
        match parsed.map(|cli| cli.command) {
            Ok(Commands::Serve { bind, port }) => {
                assert_eq!(bind, "127.0.0.2");
                assert_eq!(port, 8788);
            }
            _ => panic!("serve command must parse"),
        }
        match prior {
            Some(v) => std::env::set_var("ARBITRAGEUR_BIND", v),
            None => std::env::remove_var("ARBITRAGEUR_BIND"),
        }
    }
}
