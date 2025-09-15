use crate::data::{filter_contract_by_prefix, load_market_data, normalize_date_to_bar};
use crate::params::Params;
use crate::strategy::PairStrategy;
// use crate::strategy::*;
use anyhow::Result;
use polars::frame::DataFrame;
use polars::prelude::*;
use polars::series::ChunkCompareEq;
use serde::Serialize;
use std::collections::HashMap;

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
}

const STARTING_CASH: f64 = 100_000.0;
/// Core engine object holding mutable simulation state to avoid long argument lists.
pub struct Engine<'a> {
    params: &'a Params,
    max_concurrent: usize,
    contract_expiry_date: HashMap<String, u32>,
    current_price: HashMap<String, f32>,
    equity: f32,
    cash: f32,
    open_positions: HashMap<String, Position>,
}

fn build_expiry_date(df: &DataFrame) -> Result<HashMap<String, u32>> {
    // Single pass computation of max Bar per Contract without cloning or lazy execution.
    let mut expiry: HashMap<String, u32> = HashMap::new();
    let contracts = df.column("Contract")?.str()?;
    let bars = df.column("Bar")?.u32()?;
    for i in 0..df.height() {
        if let (Some(c), Some(b)) = (contracts.get(i), bars.get(i)) {
            let entry = expiry.entry(c.to_string()).or_insert(b);
            if b > *entry {
                *entry = b;
            }
        }
    }
    Ok(expiry)
}

#[derive(Clone, PartialEq, Debug)]
pub enum PositionKind {
    Long,
    Short,
}

pub struct Position {
    pub kind: PositionKind,
    pub entry_price: f32,
    pub size: u32,
}

pub struct AccountStatus<'a> {
    pub cash: f32,
    pub equity: f32,
    pub positions: &'a HashMap<String, Position>,
}

pub trait Broker {
    fn buy(&mut self, symbol: &str, qty: u32);
    fn sell(&mut self, symbol: &str, qty: u32);

    fn get_status(&self) -> AccountStatus;
}

impl<'a> Broker for Engine<'a> {
    fn buy(&mut self, symbol: &str, qty: u32) {
        match self.current_price.get(symbol) {
            Some(price) => {
                self.cash -= price * qty as f32;
                let pos = self
                    .open_positions
                    .entry(symbol.to_string())
                    .or_insert(Position {
                        kind: PositionKind::Long,
                        entry_price: *price,
                        size: 0,
                    });
                pos.size += qty;
            }
            None => panic!("Current price for symbol {} not set", symbol),
        }
    }

    fn sell(&mut self, symbol: &str, qty: u32) {
        match self.current_price.get(symbol) {
            Some(price) => {
                self.cash += price * qty as f32;
                if let Some(pos) = self.open_positions.get_mut(symbol) {
                    pos.size -= qty;
                    if pos.size == 0 {
                        self.open_positions.remove(symbol);
                    }
                }
            }
            None => panic!("Current price for symbol {} not set", symbol),
        }
    }

    fn get_status(&self) -> AccountStatus {
        AccountStatus {
            cash: self.cash,
            equity: self.equity,
            positions: &self.open_positions,
        }
    }
}

impl<'a> Engine<'a> {
    /// Create a new Engine with empty state.
    fn new(params: &'a Params) -> Engine<'a> {
        Self {
            params,
            max_concurrent: 0,
            contract_expiry_date: HashMap::new(),
            equity: 0.0,
            cash: 0.0,
            current_price: HashMap::new(),
            open_positions: HashMap::new(),
        }
    }

    fn prepare_data_today(
        self: &Self,
        df: &DataFrame,
        day: u32,
    ) -> Result<(Vec<ContractData>, Vec<ContractData>)> {
        let mut a_contracts: Vec<ContractData> = Vec::new();
        let mut b_contracts: Vec<ContractData> = Vec::new();
        // Filter rows where Bar == day
        let mask = df.column("Bar")?.u32()?.equal(day);
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
            };
            if contract.starts_with(&self.params.commodity_a_prefix) {
                a_contracts.push(contract_data);
            } else {
                b_contracts.push(contract_data);
            }
        }
        Ok((a_contracts, b_contracts))
    }

    pub fn run(self: &mut Self, df: &DataFrame) -> Result<()> {
        let mut trading_days: Vec<u32> = df
            .column("Bar")?
            .u32()?
            .unique()?
            .into_iter()
            .filter_map(|x| x)
            .collect();
        trading_days.sort();
        let contract_expiry_date = build_expiry_date(df)?;
        let mut strategy = PairStrategy::new(self.params, contract_expiry_date);
        for day in trading_days {
            let (a_today, b_today) = self.prepare_data_today(&df, day)?;
            // Pass mutable self as broker only during the trade call; no internal storage to avoid aliasing.
            strategy.trade(day, a_today, b_today, self)?;
        }
        Ok(())
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

    let mut engine = Engine::new(params);
    engine.run(&df)?;
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
