use crate::data::{filter_contract_by_prefix, load_market_data, normalize_date_to_bar};
use crate::params::Params;
// use crate::strategy::*;
use anyhow::Result;
use chrono::NaiveDate;
use polars::frame::DataFrame;
use polars::prelude::{col, ChunkCompare, ChunkUnique};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct TradeLogEntry {
    pub pair: String,
    pub kind: String,
    pub entry_bar: usize,
    pub exit_bar: usize,
    pub ret: f64,
    pub size: u32,
    pub entry_spread: f64,
    pub exit_spread: f64,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct EngineResult {
    pub total_return: f64,
    pub sharpe_ratio: f64,
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f64,
    pub max_drawdown: f64,
    pub final_value: f64,
    pub max_concurrent_positions: usize,
    pub trade_log: Vec<TradeLogEntry>,
}

#[derive(Debug, Serialize, Clone)]
pub struct PairPerf {
    pub trades: usize,
    pub wins: usize,
    pub total_return: f64,
    pub sharpe: f64,
    pub success_score: f64,
}

const STARTING_CASH: f64 = 100_000.0;
/// Core engine object holding mutable simulation state to avoid long argument lists.
struct Engine<'a> {
    params: &'a Params,
    trade_log: Vec<TradeLogEntry>,
    bar_count: usize,
    max_concurrent: usize,
    daily_volume: HashMap<String, u32>,
}

impl<'a> Engine<'a> {
    /// Create a new Engine with empty state.
    fn new(params: &'a Params) -> Self {
        let single = params.commodity_a_prefix == params.commodity_b_prefix;
        Self {
            params,
            trade_log: Vec::new(),
            bar_count: 0,
            max_concurrent: 0,
            daily_volume: HashMap::new(),
        }
    }
}

fn calc_sharpe(equity_curve: &[f64]) -> f64 {
    let returns: Vec<f64> = equity_curve
        .windows(2)
        .map(|w| (w[1] - w[0]) / w[0]) // daily/periodic returns
        .collect();
    let mean = returns.iter().copied().sum::<f64>() / returns.len() as f64;
    let var = returns
        .iter()
        .map(|r| {
            let d = r - mean;
            d * d
        })
        .sum::<f64>()
        / returns.len() as f64;
    let std = var.sqrt();

    let sharpe = if std > 0.0 { mean / std } else { 0.0 };
    sharpe
}

pub type ContractsToday = Vec<ContractData>;
#[derive(Clone, Debug)]
pub struct ContractData {
    pub name: String,
    pub price: f32,
    pub volume: u32,
    pub oi: u32,
    pub expiry_date: u32,
}

fn build_expiry_date(md: &DataFrame) -> HashMap<String, u32> {
    let mut contract_expiry_date: HashMap<String, u32> = HashMap::new();

    contract_expiry_date
}

fn prepare_data_today(
    df: &DataFrame,
    day: u32,
    contract_expiry_date: &HashMap<String, u32>,
) -> Result<(Vec<ContractData>, Vec<ContractData>)> {
    let mut a_contracts: Vec<ContractData> = Vec::new();
    let mut b_contracts: Vec<ContractData> = Vec::new();
    // Filter rows where Bar == day
    let mask = df.column("Bar")?.equal(day)?;
    let today_df = df.filter(&mask)?;

    let contracts = today_df.column("Contract")?.str()?;
    let closes = today_df.column("Close")?.f64()?;
    let vols = today_df.column("Volume")?.f64()?;
    let ois = today_df.column("OI")?.f64()?;
    // Collect today's contracts for each commodity
    for i in 0..today_df.height() {
        let contract = contracts.get(i).unwrap();
        let close = closes.get(i).unwrap();
        let vol = vols.get(i).unwrap();
        let oi = ois.get(i).unwrap();
        let contract_data = ContractData {
            name: contract.to_string(),
            price: close as f32,
            volume: vol as u32,
            oi: oi as u32,
            expiry_date: *contract_expiry_date.get(contract).unwrap_or(&0) as u32,
        };
        if contract.starts_with(&params.commodity_a_prefix) {
            a_contracts.push(contract_data);
        } else {
            b_contracts.push(contract_data);
        }
    }
    Ok((a_contracts, b_contracts))
}

pub fn run_engine(path: &str, params: &Params) -> Result<()> {
    let df = load_market_data(path)?;
    // Filter the market data to only include contracts with the specified prefixes
    let df = filter_contract_by_prefix(df, &params.commodity_a_prefix, &params.commodity_b_prefix)?;
    let df = normalize_date_to_bar(df)?;
    if params.debug {
        println!(
            "Loaded DataFrame: rows={}, cols={}",
            df.height(),
            df.width()
        );
    }

    let date_series = df.column("Bar")?.u32()?;

    let contract_expiry_date = build_expiry_date(&df);
    let mut trading_days: Vec<u32> = date_series
        .unique()?
        .into_iter()
        .filter_map(|x| x)
        .collect();
    trading_days.sort();

    let mut engine = Engine::new(params);
    // let strategy = PairStrategy::new();

    for day in trading_days {
        let (a_today, b_today) = prepare_data_today(&df, day, &contract_expiry_date)?;
        // strategy.trade(engine.bar_count as u32, a_today, b_today);
    }
    Ok(())
}

// Aggregate statistics
// let total_trades = engine.trade_log.len();
// let winning_trades = engine.trade_log.iter().filter(|t| t.ret > 0.0).count();
// let losing_trades = total_trades - winning_trades;
// let win_rate = if total_trades > 0 {
//     winning_trades as f64 / total_trades as f64
// } else {
//     0.0
// };

// // Sharpe on equity changes per bar
// let sharpe_ratio = calc_sharpe(&engine.equity_curve);

// // Max drawdown
// let mut peak = f64::MIN;
// let mut max_dd = 0.0;
// for v in &engine.equity_curve {
//     if *v > peak {
//         peak = *v;
//     }
//     let dd = peak - *v;
//     if dd > max_dd {
//         max_dd = dd;
//     }
// }

// // Pair performance export
// let mut pair_performance: HashMap<String, PairPerf> = HashMap::new();
// for ((c1, c2), ps) in engine.pair_stats.into_iter() {
//     pair_performance.insert(
//         format!("{}/{}", c1, c2),
//         PairPerf {
//             trades: ps.trades,
//             wins: ps.wins,
//             total_return: ps.total_return,
//             sharpe: ps.sharpe,
//             success_score: ps.success_score,
//         },
//     );
// }

// Sort trade log so the largest magnitude winners/losers appear first (big losses & big gains).
// engine.trade_log.sort_by(|a, b| {
//     b.ret
//         .abs()
//         .partial_cmp(&a.ret.abs())
//         .unwrap_or(Ordering::Equal)
// });

// Ok(EngineResult {
//     final_value: engine.equity,
//     sharpe_ratio,
//     total_trades,
//     winning_trades,
//     losing_trades,
//     win_rate,
//     max_drawdown: max_dd,
//     total_return: engine.equity - STARTING_CASH,
//     max_concurrent_positions: engine.max_concurrent,
//     pair_performance,
//     trade_log: engine.trade_log,
// })
// }

// #[cfg(test)]
// mod tests {
//     use super::*;

//     fn mk_pos(entry_spread: f64, kind: PositionKind) -> Position {
//         Position {
//             pair: ("a".into(), "b".into()),
//             kind,
//             entry_z: 0.0,
//             entry_bar: 0,
//             entry_spread,
//             size: 1,
//             trade_id: 1,
//         }
//     }

//     #[test]
//     fn percent_moved_long_negative_entry_to_positive_exit() {
//         // Case from user report: entry -210 -> exit 105, long spread raw move = 315; size=1 => PnL 315
//         let pos = mk_pos(-210.0, PositionKind::LongSpread);
//         let (raw_gain, trade_ret) = close_trade(&pos, 105.0);
//         // For a long spread we store raw_gain = cur_price * size (105)
//         assert!(
//             (raw_gain - 105.0).abs() < 1e-9,
//             "expected raw_gain 105 got {}",
//             raw_gain
//         );
//         assert!(
//             (trade_ret - 315.0).abs() < 1e-9,
//             "expected 315 trade_ret got {}",
//             trade_ret
//         );
//     }

//     #[test]
//     fn percent_moved_short_positive_move_down() {
//         // Short spread: entry 500 -> exit 350, raw directional move = 150 (profit); size=1 => PnL 150
//         let pos = mk_pos(500.0, PositionKind::ShortSpread);
//         let (raw_gain, trade_ret) = close_trade(&pos, 350.0);
//         // For a short spread raw_gain = -cur_price * size (-350)
//         assert!(
//             (raw_gain + 350.0).abs() < 1e-9,
//             "expected raw_gain -350 got {}",
//             raw_gain
//         );
//         assert!(
//             (trade_ret - 150.0).abs() < 1e-9,
//             "expected 150 trade_ret got {}",
//             trade_ret
//         );
//     }

//     #[test]
//     fn percent_moved_zero_entry_protected() {
//         let pos = mk_pos(0.0, PositionKind::LongSpread);
//         let (raw_gain, trade_ret) = close_trade(&pos, 10.0);
//         assert!(
//             (raw_gain - 10.0).abs() < 1e-9,
//             "expected raw_gain 10 got {}",
//             raw_gain
//         );
//         // entry spread zero -> move = 10 - 0 = 10; still valid
//         assert_eq!(trade_ret, 10.0);
//     }

//     #[test]
//     fn trade_return_long_negative_spread() {
//         // Entry spread -210, exit +105 -> raw move = 315, size=1 => return 315
//         let pos = mk_pos(-210.0, PositionKind::LongSpread);
//         let (raw_gain, trade_ret) = close_trade(&pos, 105.0);
//         assert!(
//             (raw_gain - 105.0).abs() < 1e-9,
//             "expected raw_gain 105 got {}",
//             raw_gain
//         );
//         assert!(
//             (trade_ret - 315.0).abs() < 1e-6,
//             "expected 315 got {}",
//             trade_ret
//         );
//     }

//     #[test]
//     fn trade_return_scales_with_size() {
//         // Same spread move as previous test but with size=4
//         let mut pos = mk_pos(-210.0, PositionKind::LongSpread);
//         pos.size = 4;
//         let (raw_gain, trade_ret) = close_trade(&pos, 105.0);
//         assert!(
//             (raw_gain - 4.0 * 105.0).abs() < 1e-9,
//             "expected raw_gain {} got {}",
//             4.0 * 105.0,
//             raw_gain
//         );
//         assert!(
//             (trade_ret - 4.0 * 315.0).abs() < 1e-6,
//             "expected {} got {}",
//             4.0 * 315.0,
//             trade_ret
//         );
//     }
// }
