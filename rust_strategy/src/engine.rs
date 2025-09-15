use crate::data::{filter_contract_by_prefix, load_market_data, normalize_date_to_bar};
use crate::params::Params;
use crate::strategy::{PairStrategy, TradeLogEntry};
// use crate::strategy::*;
use anyhow::Result;
use polars::frame::DataFrame;
use polars::prelude::*;
use polars::series::ChunkCompareEq;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct BackTestResult {
    pub total_return: f32,
    pub sharpe_ratio: f32,
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f32,
    pub max_drawdown: f32,
    pub final_value: f32,
    pub max_concurrent_positions: usize,
    pub trade_log: Vec<TradeLogEntry>,
}

const STARTING_CASH: f32 = 100_000.0;
/// Core engine object holding mutable simulation state to avoid long argument lists.
pub struct Engine<'a> {
    params: &'a Params,
    current_price: HashMap<String, f32>,
    equity_curve: Vec<f32>,
    equity: f32,
    cash: f32,
    open_positions: HashMap<String, Position>,
    max_concurrent_positions: usize,
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
    pub entry_price: f32,
    pub size: i32,
}

pub type ContractsToday = Vec<ContractData>;
#[derive(Clone, Debug)]
pub struct ContractData {
    pub name: String,
    pub price: f32,
    pub volume: u32,
}

pub struct AccountStatus {
    pub cash: f32,
    pub equity: f32,
}

pub trait Broker {
    fn buy(&mut self, symbol: &str, qty: u32) -> Option<i32>;
    fn sell(&mut self, symbol: &str, qty: u32) -> Option<i32>;

    fn get_status(&'_ self) -> AccountStatus;
}

impl<'a> Broker for Engine<'a> {
    fn buy(&mut self, symbol: &str, qty: u32) -> Option<i32> {
        self.trade(symbol, qty as i32)
    }
    fn sell(&mut self, symbol: &str, qty: u32) -> Option<i32> {
        self.trade(symbol, -(qty as i32))
    }

    fn get_status(&'_ self) -> AccountStatus {
        AccountStatus {
            cash: self.cash,
            equity: self.equity,
        }
    }
}

impl<'a> Engine<'a> {
    /// Create a new Engine with empty state.
    fn new(params: &'a Params) -> Engine<'a> {
        Self {
            params,
            equity_curve: Vec::new(),
            equity: STARTING_CASH,
            cash: STARTING_CASH,
            current_price: HashMap::new(),
            open_positions: HashMap::new(),
            max_concurrent_positions: 0,
        }
    }

    fn trade(&mut self, symbol: &str, qty: i32) -> Option<i32> {
        let cur_price = self.current_price.get(symbol)?;
        self.cash -= cur_price * qty as f32;
        let pos = self
            .open_positions
            .entry(symbol.to_string())
            .or_insert(Position {
                entry_price: *cur_price,
                size: 0,
            });
        pos.size += qty;
        let new_size = pos.size;
        if new_size == 0 {
            self.open_positions.remove(symbol);
        }

        if self.open_positions.len() > self.max_concurrent_positions {
            self.max_concurrent_positions = self.open_positions.len();
        }
        Some(new_size)
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
        let closes = today_df.column("Close")?.f32()?;
        let vols = today_df.column("Volume")?.f32()?;
        // Collect today's contracts for each commodity
        for i in 0..today_df.height() {
            let contract = contracts.get(i).unwrap();
            let close = closes.get(i).unwrap();
            let vol = vols.get(i).unwrap();
            let contract_data = ContractData {
                name: contract.to_string(),
                price: close as f32,
                volume: vol as u32,
            };
            if contract.starts_with(&self.params.commodity_a_prefix) {
                a_contracts.push(contract_data);
            } else {
                b_contracts.push(contract_data);
            }
        }
        Ok((a_contracts, b_contracts))
    }

    fn update_equity(self: &mut Self) {
        let position_value: f32 = self
            .open_positions
            .iter()
            .filter_map(|(symbol, pos)| {
                self.current_price
                    .get(symbol)
                    .map(|price| price * pos.size as f32)
            })
            .sum();
        self.equity = self.cash + position_value;
        self.equity_curve.push(self.equity);
    }

    pub fn run(self: &mut Self, df: &DataFrame) -> Result<BackTestResult> {
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
            self.current_price = a_today
                .iter()
                .chain(b_today.iter())
                .map(|c| (c.name.clone(), c.price))
                .collect();
            self.update_equity();

            strategy.trade(day, a_today, b_today, self)?;
        }

        let trades = strategy.move_log();
        let won_trade = trades.iter().filter(|t| t.ret > 0.0).count();
        Ok(BackTestResult {
            total_return: self.equity - STARTING_CASH,
            sharpe_ratio: calc_sharpe(&self.equity_curve),
            total_trades: trades.len(),
            winning_trades: won_trade,
            losing_trades: trades.len() - won_trade,
            win_rate: if trades.len() > 0 {
                won_trade as f32 / trades.len() as f32
            } else {
                0.0
            },
            max_drawdown: self.calc_drawdown(),
            final_value: self.equity,
            max_concurrent_positions: self.max_concurrent_positions,
            trade_log: trades,
        })
    }

    fn calc_drawdown(self: &Self) -> f32 {
        let mut peak = f32::MIN;
        let mut max_dd = 0.0;
        for v in &self.equity_curve {
            if *v > peak {
                peak = *v;
            }
            let dd = peak - *v;
            if dd > max_dd {
                max_dd = dd;
            }
        }
        max_dd
    }
}

fn calc_sharpe(equity_curve: &[f32]) -> f32 {
    let returns: Vec<f32> = equity_curve
        .windows(2)
        .map(|w| (w[1] - w[0]) / w[0]) // daily/periodic returns
        .collect();
    let mean = returns.iter().copied().sum::<f32>() / returns.len() as f32;
    let var = returns
        .iter()
        .map(|r| {
            let d = r - mean;
            d * d
        })
        .sum::<f32>()
        / returns.len() as f32;
    let std = var.sqrt();

    let sharpe = if std > 0.0 { mean / std } else { 0.0 };
    sharpe
}

pub fn run_engine(path: &str, params: &Params) -> Result<BackTestResult> {
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
    engine.run(&df)
}
// Aggregate statistics
// let total_trades = engine.trade_log.len();
// let winning_trades = engine.trade_log.iter().filter(|t| t.ret > 0.0).count();
// let losing_trades = total_trades - winning_trades;
// let win_rate = if total_trades > 0 {
//     winning_trades as f32 / total_trades as f32
// } else {
//     0.0
// };

// // Sharpe on equity changes per bar
// let sharpe_ratio = calc_sharpe(&engine.equity_curve);

// // Max drawdown
// let mut peak = f32::MIN;
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

//     fn mk_pos(entry_spread: f32, kind: PositionKind) -> Position {
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
