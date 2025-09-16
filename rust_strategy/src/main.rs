mod data;
mod engine;
mod params;
mod strategy;

use anyhow::Result;
use clap::Parser;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde::Serialize;

// use crate::engine::run_engine;
use crate::{engine::run_engine, params::Params};

#[derive(Serialize, Clone)]
struct MultiPairResultEntry {
    commodity_a: String,
    commodity_b: String,
    total_return: f32,
    sharpe_ratio: f32,
    win_rate: f32,
    total_trades: usize,
    max_drawdown: f32,
}

#[derive(Serialize)]
struct MultiPairAggregate {
    pairs: Vec<MultiPairResultEntry>,
    ranked_by_sharpe: Vec<String>,
    ranked_by_return: Vec<String>,
}

#[derive(Parser, Debug)]
#[command(
    name = "rust_strategy",
    about = "Dynamic Generic Commodity Pair Strategy (Rust)"
)]
struct Cli {
    /// Path to parquet file or directory of parquet files
    #[arg(long = "data", short = 'd', default_value = "../data")]
    data: String,

    /// Z-score lookback window
    #[arg(long = "lookback-zscore", short = 'l', default_value_t = 20)]
    lookback_zscore: usize,

    /// Entry z-score threshold
    #[arg(long = "entry-z", short = 'e', default_value_t = 2.0)]
    entry_z: f32,

    /// Exit z-score threshold
    #[arg(long = "exit-z", short = 'x', default_value_t = 0.5)]
    exit_z: f32,

    /// Enable debug logging
    #[arg(long = "debug", default_value_t = false)]
    debug: bool,

    /// Days before a contract's final trading day to force-close positions
    #[arg(long = "expiry-close-days", default_value_t = 3)]
    expiry_close_days: u32,

    /// Commodity A prefix (e.g. cu) for single-run mode
    #[arg(short = 'a', default_value = "ag")]
    commodity_a: String,

    /// Commodity B prefix (e.g. fu) for single-run mode
    #[arg(short = 'b', default_value = "au")]
    commodity_b: String,

    /// Path to a text file containing commodity prefix pairs (one a:b per line; lines starting with # ignored)
    #[arg(long = "pairs-file")]
    pairs_file: Option<String>,

    /// Optional output JSON file path (if omitted, only stdout is used)
    #[arg(long = "out", short = 'o', default_value = "output.json")]
    out: Option<String>,
}

fn build_params(cli: &Cli, a: &str, b: &str) -> Params {
    // Canonicalize ordering so (a,b) and (b,a) yield identical strategy behavior.
    let (a_canon, b_canon) = if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    };
    Params {
        lookback_zscore: cli.lookback_zscore,
        entry_z: cli.entry_z,
        exit_z: cli.exit_z,
        expiry_close_days: cli.expiry_close_days,
        debug: cli.debug,
        commodity_a_prefix: a_canon,
        commodity_b_prefix: b_canon,
        transaction_cost_pct: 0.0001, // default unless extended CLI adds option
    }
}

fn parse_pairs(spec: &str) -> Vec<(String, String)> {
    spec.split(',')
        .filter_map(|pair| {
            let mut parts = pair.split(':');
            let a = parts.next()?.trim();
            let b = parts.next()?.trim();
            if a.is_empty() || b.is_empty() {
                return None;
            }
            Some((a.to_string(), b.to_string()))
        })
        .collect()
}

fn load_pairs_from_file(path: &str) -> Result<Vec<(String, String)>> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut pairs = Vec::new();
    for line_res in reader.lines() {
        let line = line_res?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // allow either comma-separated line of pairs or single a:b
        if trimmed.contains(',') {
            pairs.extend(parse_pairs(trimmed));
        } else {
            let mut parts = trimmed.split(':');
            if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                let a = a.trim();
                let b = b.trim();
                if !a.is_empty() && !b.is_empty() {
                    pairs.push((a.to_string(), b.to_string()));
                }
            }
        }
    }
    Ok(pairs)
}

fn test_multiple_pairs(cli: &Cli, collected: Vec<(String, String)>) -> Result<()> {
    // Parallel execution: each pair independent. Collect results; if any fail, propagate first error.
    let entries: Vec<MultiPairResultEntry> = collected
        .par_iter()
        .map(|(a, b)| {
            let params = build_params(&cli, a, b);
            let res = run_engine(&cli.data, &params);
            (params, res)
        })
        .map(|(params, res)| match res {
            Ok(r) => Ok(MultiPairResultEntry {
                commodity_a: params.commodity_a_prefix,
                commodity_b: params.commodity_b_prefix,
                total_return: r.total_return,
                sharpe_ratio: r.sharpe_ratio,
                win_rate: r.win_rate,
                total_trades: r.total_trades,
                max_drawdown: r.max_drawdown,
            }),
            Err(e) => Err(e),
        })
        .collect::<Result<Vec<_>>>()?;

    let candidates: Vec<MultiPairResultEntry> = entries
        .into_iter()
        .filter(|e| e.total_return > 0.0 && e.sharpe_ratio > 0.0)
        .collect();
    if candidates.is_empty() {
        eprintln!("No pairs passed the non-negative return & Sharpe filter");
        return Ok(());
    }

    let mut by_sharpe = candidates.clone();
    by_sharpe.sort_by(|x, y| {
        y.sharpe_ratio
            .partial_cmp(&x.sharpe_ratio)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let ranked_by_sharpe = by_sharpe
        .iter()
        .map(|e| format!("{}:{}", e.commodity_a, e.commodity_b))
        .collect();

    let ranked_by_return = {
        let mut v = candidates.clone();
        v.sort_by(|x, y| {
            y.total_return
                .partial_cmp(&x.total_return)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v.iter()
            .map(|e| format!("{}:{}", e.commodity_a, e.commodity_b))
            .collect::<Vec<_>>()
    };

    let aggregate = MultiPairAggregate {
        pairs: candidates,
        ranked_by_sharpe,
        ranked_by_return,
    };
    let json = serde_json::to_string_pretty(&aggregate)?;
    if let Some(path) = cli.out.as_ref() {
        std::fs::write(path, json)?;
        println!("Wrote output JSON to {}", path);
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(file_path) = &cli.pairs_file {
        let pairs =  load_pairs_from_file(file_path)?;
        test_multiple_pairs(&cli, pairs)?;
    } else {
        // test a single pair
        let params = build_params(&cli, &cli.commodity_a, &cli.commodity_b);
        let result = run_engine(&cli.data, &params)?;
        println!("Total Return = {}", result.total_return);
        println!("Sharpe Ratio = {}", result.sharpe_ratio);
        println!("Win Rate = {}", result.win_rate);
        let json = serde_json::to_string_pretty(&result)?;
        if let Some(path) = cli.out.as_ref() {
            std::fs::write(path, json)?;
            println!("Wrote output JSON to {}", path);
        }
    }
    Ok(())
}
