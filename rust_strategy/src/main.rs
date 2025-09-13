mod params;
mod data;
mod stats;
mod engine;

use clap::Parser;
use anyhow::Result;
use serde::Serialize;

use crate::params::Params;
use crate::engine::run_engine;

#[derive(Serialize)]
struct OutputSummary {
    total_return: f64,
    sharpe_ratio: f64,
    total_trades: usize,
    winning_trades: usize,
    losing_trades: usize,
    win_rate: f64,
    max_drawdown: f64,
    final_value: f64,
}

#[derive(Parser, Debug)]
#[command(name = "rust_strategy", about = "Dynamic Copper-Fuel Oil Pairs Strategy (Rust)")]
struct Cli {
    /// Path to parquet file or directory of parquet files
    #[arg(long = "data", short = 'd')]
    data: String,

    /// Z-score lookback window
    #[arg(long = "lookback-zscore", short = 'l', default_value_t = 20)]
    lookback_zscore: usize,

    /// Entry z-score threshold
    #[arg(long = "entry-z", short = 'e', default_value_t = 2.0)]
    entry_z: f64,

    /// Exit z-score threshold
    #[arg(long = "exit-z", short = 'x', default_value_t = 0.5)]
    exit_z: f64,

    /// Maximum active pairs
    #[arg(long = "max-pairs", short = 'p', default_value_t = 3)]
    max_pairs: usize,

    /// Pair evaluation frequency
    #[arg(long = "eval-freq", short = 'f', default_value_t = 10)]
    eval_freq: usize,

    /// Enable debug logging
    #[arg(long = "debug", default_value_t = false)]
    debug: bool,

    /// Days before a contract's final trading day to force-close positions
    #[arg(long = "expiry-close-days", default_value_t = 3)]
    expiry_close_days: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let params = Params {
        lookback_zscore: cli.lookback_zscore,
        entry_z: cli.entry_z,
        exit_z: cli.exit_z,
        pair_evaluation_freq: cli.eval_freq,
        max_active_pairs: cli.max_pairs,
        lookback_performance: 50,
        exploration_rate: 0.2,
        min_volume_threshold: 50,
        expiry_close_days: cli.expiry_close_days,
        debug: cli.debug,
    };

    let result = run_engine(&cli.data, &params)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
