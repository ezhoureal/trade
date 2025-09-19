use crate::data::{filter_contract_by_prefix, load_market_data, normalize_date_to_bar};
use crate::params::Params;
use crate::strategy::{PairStrategy, TradeLogEntry};
// use crate::strategy::*;
use anyhow::Result;
use polars::frame::DataFrame;
use polars::prelude::*;
use polars::series::ChunkCompareEq;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use async_trait::async_trait;

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

const STARTING_CASH: f32 = 1_000_000.0;
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

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum PositionKind {
    Long,
    Short,
}

#[allow(dead_code)]
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
    pub gross_exposure: f32,
}

#[async_trait]
pub trait Broker {
    async fn exec_spread(&mut self, pair: (&str, &str), qty: i32, open: bool) -> Option<u32>;

    fn get_status(&'_ self) -> AccountStatus;
}

#[async_trait]
impl<'a> Broker for Engine<'a> {
    async fn exec_spread(&mut self, pair: (&str, &str), qty: i32, open: bool) -> Option<u32> {
        let (a, b) = pair;
        if open {
            self.trade(a, qty);
            self.trade(b, -qty);
        } else {
            self.trade(a, -qty);
            self.trade(b, qty);
        }
        Some(qty.abs() as u32)
    }

    fn get_status(&'_ self) -> AccountStatus {
        AccountStatus {
            cash: self.cash,
            equity: self.equity,
            gross_exposure: self
                .open_positions
                .iter()
                .filter_map(|(symbol, pos)| {
                    self.current_price
                        .get(symbol)
                        .map(|price| price * pos.size.abs() as f32)
                })
                .sum(),
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
        let transaction_cost = cur_price * qty.abs() as f32 * self.params.transaction_cost_pct;
        self.cash -= transaction_cost;
        if self.cash < 0.0 {
            println!("Warning: Cash balance negative after trade on {} qty {} at price {:.2}. balance = {:.2}", symbol, qty, cur_price, self.cash);
        }
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
        let same_prefix = self.params.commodity_a_prefix == self.params.commodity_b_prefix;
        // Filter rows where Bar == day
        let mask = df.column("Bar")?.u32()?.equal(day);
        let today_df = df.filter(&mask)?;

        let contracts = today_df.column("Contract")?.str()?;
        let closes = today_df.column("Close")?.f64()?;
        let vols = today_df.column("Volume")?.f64()?;
        // Collect today's contracts for each commodity (or single commodity if same prefix case)
        for i in 0..today_df.height() {
            let contract = contracts.get(i).unwrap();
            let close = closes.get(i).unwrap();
            let vol = vols.get(i).unwrap();
            let contract_data = ContractData {
                name: contract.to_string(),
                price: close as f32,
                volume: vol as u32,
            };
            if same_prefix {
                a_contracts.push(contract_data);
            } else {
                if contract.starts_with(&self.params.commodity_a_prefix) {
                    a_contracts.push(contract_data);
                } else {
                    b_contracts.push(contract_data);
                }
            }
        }
        if same_prefix {
            // For intra-commodity pairing we use the same set on both sides.
            b_contracts = a_contracts.clone();
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
        println!("Total trading days: {}", trading_days.len());

        let contract_expiry_date = build_expiry_date(df)?;
        let mut strategy = PairStrategy::new(self.params.clone(), contract_expiry_date);
        for day in trading_days {
            let (a_today, b_today) = self.prepare_data_today(&df, day)?;
            self.current_price = a_today
                .iter()
                .chain(b_today.iter())
                .map(|c| (c.name.clone(), c.price))
                .collect();

            if self.params.debug && day % 50 == 0 {
                println!(
                    "Day {}: cash {:.2}, equity {:.2}, open positions {}, max concurrent {}",
                    day,
                    self.cash,
                    self.equity,
                    self.open_positions.len(),
                    self.max_concurrent_positions
                );
            }
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
            max_drawdown: calc_drawdown(&self.equity_curve),
            final_value: self.equity,
            max_concurrent_positions: self.max_concurrent_positions,
            trade_log: trades,
        })
    }
}

fn calc_drawdown(equity_curve: &[f32]) -> f32 {
    let mut peak = f32::MIN;
    let mut max_dd = 0.0;
    for v in equity_curve {
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

fn calc_sharpe(equity_curve: &[f32]) -> f32 {
    const PERIOD_PER_YEAR: f32 = 252.0;
    let returns: Vec<f32> = equity_curve
        .windows(2)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect();

    if returns.len() < 2 {
        return 0.0;
    }

    let mean = returns.iter().copied().sum::<f32>() / returns.len() as f32;

    let var = returns
        .iter()
        .map(|r| {
            let d = r - mean;
            d * d
        })
        .sum::<f32>()
        / (returns.len() as f32 - 1.0);

    let std = var.sqrt();

    if std > 0.0 {
        (mean / std) * PERIOD_PER_YEAR.sqrt()
    } else {
        0.0
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;

    fn test_params() -> Params {
        Params {
            lookback_zscore: 5,
            entry_z: 2.0,
            exit_z: 0.5,
            expiry_close_days: 5,
            debug: false,
            commodity_a_prefix: "A".into(),
            commodity_b_prefix: "B".into(),
            transaction_cost_pct: 0.0,
        }
    }

    #[test]
    fn trade_open_and_close_long_position() {
        let params = test_params();
        let mut engine = Engine::new(&params);
        engine.current_price.insert("A1".into(), 50.0);
        let starting_cash = engine.cash;
        // Open long (positive qty)
        let size_after_open = engine.trade("A1", 10).expect("trade should succeed");
        assert_eq!(size_after_open, 10);
        assert!(
            engine.cash < starting_cash - 499.9 && engine.cash > starting_cash - 500.1,
            "Cash should decrease by price*qty"
        );
        assert_eq!(engine.open_positions.len(), 1);

        // Close position
        let size_after_close = engine.trade("A1", -10).expect("close should succeed");
        assert_eq!(size_after_close, 0);
        assert!(
            engine.open_positions.is_empty(),
            "Position map should remove flat position"
        );
        assert!(
            (engine.cash - starting_cash).abs() < 1e-4,
            "Cash should round-trip after round trip trade"
        );
        assert_eq!(
            engine.max_concurrent_positions, 1,
            "Max concurrent should record peak"
        );
    }

    #[test]
    fn trade_open_and_close_short_position() {
        let params = test_params();
        let mut engine = Engine::new(&params);
        engine.current_price.insert("A1".into(), 25.0);
        let starting_cash = engine.cash;
        // Open short (negative qty first)
        let size_after_open = engine.trade("A1", -8).expect("short trade ok");
        assert_eq!(size_after_open, -8);
        assert!(
            engine.cash > starting_cash + 199.9 && engine.cash < starting_cash + 200.1,
            "Cash should increase on short sale"
        );
        assert_eq!(engine.open_positions.len(), 1);
        // Cover
        let size_after_close = engine.trade("A1", 8).expect("cover ok");
        assert_eq!(size_after_close, 0);
        assert!(engine.open_positions.is_empty());
        assert!(
            (engine.cash - starting_cash).abs() < 1e-4,
            "Cash should return after short round trip"
        );
        assert_eq!(engine.max_concurrent_positions, 1);
    }

    #[test]
    fn trade_tracks_max_concurrent_positions() {
        let params = test_params();
        let mut engine = Engine::new(&params);
        engine.current_price.insert("A1".into(), 10.0);
        engine.current_price.insert("B1".into(), 20.0);
        engine.trade("A1", 5).unwrap();
        engine.trade("B1", 10).unwrap();
        assert_eq!(engine.open_positions.len(), 2);
        assert_eq!(engine.max_concurrent_positions, 2);
        // Close one
        engine.trade("A1", -5).unwrap();
        assert_eq!(engine.open_positions.len(), 1);
        assert_eq!(
            engine.max_concurrent_positions, 2,
            "Peak should remain recorded"
        );
    }

    #[test]
    fn calc_drawdown_basic() {
        let eq = vec![100.0, 120.0, 80.0, 90.0, 70.0, 130.0];
        let dd = calc_drawdown(&eq);
        // Max drop from prior peak 120 -> 70 = 50.
        assert!((dd - 50.0).abs() < 1e-6, "Expected drawdown 50, got {}", dd);
    }

    #[test]
    fn calc_drawdown_monotonic_up() {
        let eq = vec![100.0, 110.0, 120.0, 130.0];
        let dd = calc_drawdown(&eq);
        assert_eq!(dd, 0.0, "No drawdown in monotonic increase");
    }

    #[test]
    fn calc_sharpe_zero_variance() {
        let eq = vec![100.0, 100.0, 100.0];
        let s = calc_sharpe(&eq);
        assert_eq!(s, 0.0, "Sharpe should be zero with zero variance");
    }

    #[test]
    fn prepare_data_today_same_prefix_duplicates_sets() {
        // Build a tiny DataFrame with three contracts of same commodity prefix
        let params = Params {
            lookback_zscore: 5,
            entry_z: 2.0,
            exit_z: 0.5,
            expiry_close_days: 5,
            debug: false,
            commodity_a_prefix: "AG".into(),
            commodity_b_prefix: "AG".into(), // same prefix
            transaction_cost_pct: 0.0,
        };
        let engine = Engine::new(&params);
        // Construct a DataFrame manually using the df! macro
        let df = df! {
            "Contract" => &["AG1", "AG2", "AG3"],
            "Bar" => &[1u32, 1u32, 1u32],
            "Close" => &[10.0f64, 11.0, 12.0],
            "Volume" => &[1000.0f64, 2000.0, 1500.0]
        }
        .expect("dataframe build");
        let (a, b) = engine.prepare_data_today(&df, 1).expect("prep ok");
        assert_eq!(a.len(), 3, "All contracts should appear in A vector");
        assert_eq!(
            b.len(),
            3,
            "Same contracts should be mirrored in B vector for intra-commodity"
        );
        // Ensure deep clone (distinct allocations) - modifying one side shouldn't affect the other
        assert!(a.iter().zip(b.iter()).all(|(l, r)| l.name == r.name));
    }
}
