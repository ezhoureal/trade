use crate::data::load_market_data;
use crate::params::Params;
use crate::stats::PairStats;
use ahash::AHasher;
use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

#[derive(Debug, Serialize)]
pub struct TradeLogEntry {
    pub pair: String,
    pub kind: String,
    pub entry_bar: usize,
    pub exit_bar: usize,
    pub entry_z: f64,
    pub exit_z: f64,
    pub ret: f64,
    pub pct_move: f64,
    pub entry_spread: f64,
    pub exit_spread: f64,
    pub reason: String,
    pub trade_id: u64,
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
    pub pair_performance: HashMap<String, PairPerf>,
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

#[allow(dead_code)] // some fields reserved for future sizing logic
#[derive(Clone)]
struct Position {
    pair: (String, String),
    kind: PositionKind,
    entry_z: f64,
    entry_bar: usize,
    entry_spread: f64,
    size: u32,
    trade_id: u64,
    capital_committed: f64,
}

#[derive(Clone, PartialEq, Debug)]
enum PositionKind {
    LongSpread,
    ShortSpread,
}

/// Core engine object holding mutable simulation state to avoid long argument lists.
struct Engine<'a> {
    params: &'a Params,
    spread_histories: HashMap<(String, String), VecDeque<f64>>,
    active_positions: HashMap<(String, String), Position>,
    pair_stats: HashMap<(String, String), PairStats>,
    trade_log: Vec<TradeLogEntry>,
    bar_count: usize,
    equity: f64,
    equity_curve: Vec<f64>,
    contract_last_day: HashMap<String, usize>,
    max_concurrent: usize,
    starting_capital: f64,
    cash: f64,
    invested_capital: f64,
}

impl<'a> Engine<'a> {
    /// Create a new Engine with empty state.
    fn new(params: &'a Params) -> Self {
        Self {
            params,
            spread_histories: HashMap::new(),
            active_positions: HashMap::new(),
            pair_stats: HashMap::new(),
            trade_log: Vec::new(),
            bar_count: 0,
            equity: 0.0,
            equity_curve: Vec::new(),
            contract_last_day: HashMap::new(),
            max_concurrent: 0,
            starting_capital: 100_000.0,
            cash: 100_000.0,
            invested_capital: 0.0,
        }
    }
}

impl<'a> Engine<'a> {
    // compute mean, std, z of last value; returns (z, last_spread)
    fn calc_z(&self, hist: &VecDeque<f64>) -> Option<(f64, f64)> {
        if hist.is_empty() {
            return None;
        }
        let len = hist.len();
        let mean = hist.iter().copied().sum::<f64>() / len as f64;
        let var = hist
            .iter()
            .map(|v| {
                let d = v - mean;
                d * d
            })
            .sum::<f64>()
            / len as f64;
        let std = var.sqrt();
        if std == 0.0 {
            return None;
        }
        let last = *hist.back()?;
        Some(((last - mean) / std, last))
    }

    fn push_spread(&mut self, pair: (String, String), spread: f64) {
        let lookback = self.params.lookback_zscore;
        let entry = self
            .spread_histories
            .entry(pair.clone())
            .or_insert_with(|| VecDeque::with_capacity(lookback));
        if entry.len() == lookback {
            entry.pop_front();
        }
        entry.push_back(spread);
        if self.params.debug && entry.len() == lookback {
            eprintln!(
                "Pair {:?} reached lookback with last spread {:.4}",
                pair, spread
            );
        }
    }

    fn close_positions(&mut self) {
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, _pos) in self.active_positions.iter() {
            if let Some(hist) = self.spread_histories.get(pair) {
                if hist.len() >= self.params.lookback_zscore {
                    if let Some((z, _)) = self.calc_z(hist) {
                        if z.abs() < self.params.exit_z || z.abs() > 3.0 {
                            to_close.push(pair.clone());
                        }
                    }
                }
            }
        }
        for pair in to_close {
            let pos = match self.active_positions.remove(&pair) {
                Some(p) => p,
                None => continue,
            };
            let hist = match self.spread_histories.get(&pair) {
                Some(h) => h,
                None => continue,
            };
            let (z, last_spread) = match self.calc_z(hist) {
                Some(v) => v,
                None => continue,
            };
            // Price-based PnL: spread move (exit - entry); invert for short spread
            let raw_diff = last_spread - pos.entry_spread;
            // Scale price spread movement by capital committed. Interpret raw_diff relative to entry_spread
            // to approximate percentage move; guard against division by near-zero.
            let pct_move = if pos.entry_spread.abs() > 1e-9 {
                raw_diff / pos.entry_spread
            } else {
                0.0
            };
            let directional = match pos.kind {
                PositionKind::LongSpread => pct_move,
                PositionKind::ShortSpread => -pct_move,
            };
            let trade_ret = directional * pos.capital_committed; // monetary PnL
            if self.params.debug && trade_ret < 0.0 {
                eprintln!("LOSS: Trade {} for pair {:?} lost {:.2}, enter spread = {:.2}, exit spread = {:.2}", pos.trade_id, pair, trade_ret, pos.entry_spread, last_spread);
            }
            self.equity += trade_ret;
            // Release capital and add PnL to cash
            self.invested_capital -= pos.capital_committed;
            self.cash += pos.capital_committed + trade_ret;
            let stats = self
                .pair_stats
                .entry(pair.clone())
                .or_insert_with(|| PairStats::new(self.params.lookback_performance));
            stats.record(trade_ret, self.bar_count);
            self.trade_log.push(TradeLogEntry {
                pair: format!("{}/{}", pair.0, pair.1),
                kind: match pos.kind {
                    PositionKind::LongSpread => "long_spread".into(),
                    PositionKind::ShortSpread => "short_spread".into(),
                },
                entry_bar: pos.entry_bar,
                exit_bar: self.bar_count,
                entry_z: pos.entry_z,
                exit_z: z,
                ret: trade_ret,
                pct_move: directional,
                entry_spread: pos.entry_spread,
                exit_spread: last_spread,
                reason: "reversion".into(),
                trade_id: pos.trade_id,
            });
        }
    }

    fn try_enter_positions(
        &mut self,
        a_contracts_today: &[(String, f64)],
        b_contracts_today: &[(String, f64)],
    ) {
        // If we've exhausted cash, we cannot open any new positions regardless of signals.
        if self.cash <= 0.0 {
            if self.params.debug {
                eprintln!(
                    "No cash available: skipping new entries (cash={:.2})",
                    self.cash
                );
            }
            return;
        }
        // Buffer (in trading days) before expiry during which we avoid opening new positions.
        const ENTRY_EXPIRY_BUFFER: usize = 7;
        let cur_day = self.bar_count;
        // Current total equity notionally: starting_capital + realized equity change (self.equity)
        let current_total_equity = self.starting_capital + self.equity;
        for (a, _oi_a) in a_contracts_today {
            for (b, _oi_b) in b_contracts_today {
                let pair = (a.clone(), b.clone());
                if self.active_positions.contains_key(&pair) {
                    continue;
                }

                // Skip if either contract will expire within the buffer window.
                let near_expiry_a = self
                    .contract_last_day
                    .get(a)
                    .map(|last| *last <= cur_day + ENTRY_EXPIRY_BUFFER)
                    .unwrap_or(false);
                let near_expiry_b = self
                    .contract_last_day
                    .get(b)
                    .map(|last| *last <= cur_day + ENTRY_EXPIRY_BUFFER)
                    .unwrap_or(false);
                if near_expiry_a || near_expiry_b {
                    if self.params.debug {
                        eprintln!(
                            "Skip entry {:?} due to impending expiry (a_expiring={}, b_expiring={})",
                            pair, near_expiry_a, near_expiry_b
                        );
                    }
                    continue;
                }
                let hist = match self.spread_histories.get(&pair) {
                    Some(h) => h,
                    None => continue,
                };
                if hist.len() < self.params.lookback_zscore {
                    continue;
                }
                let (z, last_spread) = match self.calc_z(hist) {
                    Some(v) => v,
                    None => continue,
                };

                if self.params.debug && z.abs() > self.params.entry_z * 0.5 {
                    eprintln!(
                        "Candidate {:?} z={:.3} (threshold {})",
                        pair, z, self.params.entry_z
                    );
                }
                if z.abs() <= self.params.entry_z {
                    continue;
                }
                // Capital allocation: up to 80% of current total equity per new position.
                // But cannot exceed remaining cash. This enforces hard cash constraint.
                let desired_allocation = 0.8 * current_total_equity;
                let allocation = desired_allocation.min(self.cash);
                // If remaining cash is extremely small (e.g., dust), skip to avoid opening meaningless position.
                if allocation <= 0.0 {
                    if self.params.debug {
                        eprintln!(
                            "Zero effective allocation for {:?}; cash={:.2}",
                            pair, self.cash
                        );
                    }
                    continue;
                }
                // Optional: require at least 1% of starting capital for practicality.
                if allocation < 0.01 * self.starting_capital {
                    if self.params.debug {
                        eprintln!(
                            "Allocation below minimum threshold for {:?}: {:.2}",
                            pair, allocation
                        );
                    }
                    continue;
                }
                let mut hasher = AHasher::default();
                format!("{}_{}", a, b).hash(&mut hasher);
                let trade_id = hasher.finish();
                let kind = if z > 0.0 {
                    PositionKind::ShortSpread
                } else {
                    PositionKind::LongSpread
                };
                if self.params.debug {
                    eprintln!("ENTER {:?} {:?} z={:.3}", kind, pair, z);
                }
                self.cash -= allocation;
                self.invested_capital += allocation;
                self.active_positions.insert(
                    pair.clone(),
                    Position {
                        pair: pair.clone(),
                        kind,
                        entry_z: z,
                        entry_bar: self.bar_count,
                        entry_spread: last_spread,
                        size: ((z.abs() * 2.0).floor() as u32).max(1).min(10),
                        trade_id,
                        capital_committed: allocation,
                    },
                );
                let cur_len = self.active_positions.len();
                if cur_len > self.max_concurrent {
                    self.max_concurrent = cur_len;
                }
            }
        }
    }

    fn close_expiring_positions(&mut self) {
        if self.contract_last_day.is_empty() {
            return;
        }
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, _pos) in self.active_positions.iter() {
            // Force close if either leg is within expiry_close_days of its last observed day
            let (ref c1, ref c2) = pair;
            let expiry_window = self.params.expiry_close_days;
            let cur_day = self.bar_count; // bar_count is 1-based per loop increment
            let needs_close = self
                .contract_last_day
                .get(c1)
                .map(|d| *d <= cur_day + expiry_window)
                .unwrap_or(false)
                || self
                    .contract_last_day
                    .get(c2)
                    .map(|d| *d <= cur_day + expiry_window)
                    .unwrap_or(false);
            if needs_close {
                to_close.push(pair.clone());
            }
        }
        if to_close.is_empty() {
            return;
        }
        for pair in to_close {
            if let Some(pos) = self.active_positions.remove(&pair) {
                // Mark reason as expiry; compute current z if possible else zero PnL (flat)
                let (z_now, last_spread_opt) = if let Some(hist) = self.spread_histories.get(&pair)
                {
                    self.calc_z(hist)
                        .map(|(z, ls)| (Some(z), Some(ls)))
                        .unwrap_or((None, None))
                } else {
                    (None, None)
                };
                let trade_ret = if let Some(last_spread) = last_spread_opt {
                    let raw_diff = last_spread - pos.entry_spread;
                    let pct_move = if pos.entry_spread.abs() > 1e-9 {
                        raw_diff / pos.entry_spread
                    } else {
                        0.0
                    };
                    let directional = match pos.kind {
                        PositionKind::LongSpread => pct_move,
                        PositionKind::ShortSpread => -pct_move,
                    };
                    directional * pos.capital_committed
                } else {
                    0.0
                };
                self.equity += trade_ret;
                self.invested_capital -= pos.capital_committed;
                self.cash += pos.capital_committed + trade_ret;
                let stats = self
                    .pair_stats
                    .entry(pair.clone())
                    .or_insert_with(|| PairStats::new(self.params.lookback_performance));
                stats.record(trade_ret, self.bar_count);
                self.trade_log.push(TradeLogEntry {
                    pair: format!("{}/{}", pair.0, pair.1),
                    kind: match pos.kind {
                        PositionKind::LongSpread => "long_spread".into(),
                        PositionKind::ShortSpread => "short_spread".into(),
                    },
                    entry_bar: pos.entry_bar,
                    exit_bar: self.bar_count,
                    entry_z: pos.entry_z,
                    exit_z: z_now.unwrap_or(pos.entry_z),
                    ret: trade_ret,
                    pct_move: if let Some(last_spread) = last_spread_opt {
                        if pos.entry_spread.abs() > 1e-9 {
                            (last_spread - pos.entry_spread) / pos.entry_spread
                                * match pos.kind {
                                    PositionKind::LongSpread => 1.0,
                                    PositionKind::ShortSpread => -1.0,
                                }
                        } else {
                            0.0
                        }
                    } else {
                        0.0
                    },
                    entry_spread: pos.entry_spread,
                    exit_spread: last_spread_opt.unwrap_or(pos.entry_spread),
                    reason: "expiry".into(),
                    trade_id: pos.trade_id,
                });
            }
        }
    }
}

// Helper to process a single dataframe row, inserting prices and optional OI ranked candidates.
// Returns None if required values (contract/close) are missing so caller can use `let _ = process_row(...);`.
fn process_row(
    contract: Option<&str>,
    close: Option<f64>,
    oi: Option<f64>,
    a_prefix: &str,
    b_prefix: &str,
    a_prices: &mut HashMap<String, f64>,
    b_prices: &mut HashMap<String, f64>,
    a_contracts_today: &mut Vec<(String, f64)>,
    b_contracts_today: &mut Vec<(String, f64)>,
) -> Option<()> {
    let contract = contract?;
    let close = close?;

    if contract.starts_with(a_prefix) {
        a_prices.insert(contract.to_string(), close);
    } else if contract.starts_with(b_prefix) {
        b_prices.insert(contract.to_string(), close);
    } else {
        return Some(()); // ignore other prefixes
    }

    if let Some(oi_val) = oi {
        if contract.starts_with(a_prefix) {
            a_contracts_today.push((contract.to_string(), oi_val));
        } else if contract.starts_with(b_prefix) {
            b_contracts_today.push((contract.to_string(), oi_val));
        }
    }
    Some(())
}

pub fn run_engine(path: &str, params: &Params) -> Result<EngineResult> {
    let md = load_market_data(path, &params.commodity_a_prefix, &params.commodity_b_prefix)?;
    if params.debug {
        eprintln!(
            "Loaded DataFrame: rows={}, cols={}",
            md.df.height(),
            md.df.width()
        );
    }
    if params.debug {
        eprintln!("Unique trading days: {}", md.trading_days.len());
    }

    // Pre-group day -> rows indices to avoid filtering repeatedly (simplified)
    // (Could optimize with polars groupby but keep simple for clarity.)
    // We'll collect row indices by date.

    let mut date_indices: HashMap<NaiveDate, Vec<usize>> = HashMap::new();
    let contract_series = md.df.column("Contract")?.str()?;
    let close_series = md.df.column("Close")?.f64()?;
    let oi_series = md.df.column("OI").ok().and_then(|s| s.f64().ok());
    let date_series = md.df.column("Date")?; // assume date type or utf8 handled earlier

    // Extract dates as NaiveDate for each row
    let mut row_dates: Vec<NaiveDate> = Vec::with_capacity(md.df.height());
    match date_series.dtype() {
        polars::prelude::DataType::Date => {
            for opt_days in date_series.date()?.into_iter() {
                if let Some(days) = opt_days {
                    let nd = NaiveDate::from_num_days_from_ce_opt(days as i32 + 719163).unwrap();
                    row_dates.push(nd);
                }
            }
        }
        _ => {}
    }

    for (idx, d) in row_dates.iter().enumerate() {
        date_indices.entry(*d).or_default().push(idx);
    }

    let mut engine = Engine::new(params);

    // Pre-compute last bar index each contract appears (by bar order = trading day index)
    let mut contract_last_day: HashMap<String, usize> = HashMap::new();
    {
        // Build mapping: for each row, record bar index (day order) as last occurrence.
        // We'll iterate trading_days with index for mapping date -> bar idx.
        let mut date_to_bar: HashMap<NaiveDate, usize> = HashMap::new();
        for (i, d) in md.trading_days.iter().enumerate() {
            date_to_bar.insert(*d, i + 1);
        }
        let contract_series_full = contract_series.clone();
        // Need Date series again; we have row_dates with indices earlier (row_dates aligns to df rows by construction)
        for (row_idx, nd) in row_dates.iter().enumerate() {
            if let Some(contract) = contract_series_full.get(row_idx) {
                if let Some(bar_idx) = date_to_bar.get(nd) {
                    contract_last_day.insert(contract.to_string(), *bar_idx);
                }
            }
        }
    }
    engine.contract_last_day = contract_last_day;

    for day in &md.trading_days {
        engine.bar_count += 1;
        let indices = if let Some(v) = date_indices.get(day) {
            v
        } else {
            continue;
        }; // no data -> skip
        if params.debug && engine.bar_count <= 5 {
            eprintln!("Day {:?}: indices={}", day, indices.len());
        }

        // Collect today's prices and OI
        let mut a_prices: HashMap<String, f64> = HashMap::new();
        let mut b_prices: HashMap<String, f64> = HashMap::new();
        let mut a_contracts_today: Vec<(String, f64)> = Vec::new();
        let mut b_contracts_today: Vec<(String, f64)> = Vec::new();

        for &i in indices {
            let contract = contract_series.get(i);
            let close = close_series.get(i);
            let oi_val = oi_series.and_then(|col| col.get(i));
            let _ = process_row(
                contract,
                close,
                oi_val,
                &params.commodity_a_prefix,
                &params.commodity_b_prefix,
                &mut a_prices,
                &mut b_prices,
                &mut a_contracts_today,
                &mut b_contracts_today,
            );
        }

        // Sort by OI and take top 3 each (if OI available). If OI absent, fallback to all seen contracts today.
            a_contracts_today
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            b_contracts_today
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if params.debug && engine.bar_count <= 5 {
            eprintln!(
                "Top a today: {:?}; Top b today: {:?}",
                a_contracts_today
                    .iter()
                    .map(|(c, _)| c)
                    .collect::<Vec<_>>(),
                b_contracts_today
                    .iter()
                    .map(|(c, _)| c)
                    .collect::<Vec<_>>()
            );
        }

        // Update spreads
        for (a, _oi_a) in &a_contracts_today {
            if let Some(&a_p) = a_prices.get(a) {
                for (b, _oi_b) in &b_contracts_today {
                    if let Some(&b_p) = b_prices.get(b) {
                        engine.push_spread((a.clone(), b.clone()), a_p - b_p);
                    }
                }
            }
        }

        // First close positions that are near expiry
        engine.close_expiring_positions();
        // Then normal mean reversion closures
        engine.close_positions();
        engine.try_enter_positions(&a_contracts_today, &b_contracts_today);
        engine.equity_curve.push(engine.equity);
    }

    // Aggregate statistics
    let total_trades = engine.trade_log.len();
    let winning_trades = engine.trade_log.iter().filter(|t| t.ret > 0.0).count();
    let losing_trades = total_trades - winning_trades;
    let win_rate = if total_trades > 0 {
        winning_trades as f64 / total_trades as f64
    } else {
        0.0
    };

    // Sharpe on equity changes per bar
    let sharpe_ratio = if engine.equity_curve.len() >= 5 {
        let mean =
            engine.equity_curve.iter().copied().sum::<f64>() / engine.equity_curve.len() as f64;
        let var = engine
            .equity_curve
            .iter()
            .map(|v| {
                let d = v - mean;
                d * d
            })
            .sum::<f64>()
            / engine.equity_curve.len() as f64;
        let std = var.sqrt();
        if std > 0.0 {
            mean / std
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Max drawdown
    let mut peak = f64::MIN;
    let mut max_dd = 0.0;
    for v in &engine.equity_curve {
        if *v > peak {
            peak = *v;
        }
        let dd = peak - *v;
        if dd > max_dd {
            max_dd = dd;
        }
    }

    // Pair performance export
    let mut pair_performance: HashMap<String, PairPerf> = HashMap::new();
    for ((c1, c2), ps) in engine.pair_stats.into_iter() {
        pair_performance.insert(
            format!("{}/{}", c1, c2),
            PairPerf {
                trades: ps.trades,
                wins: ps.wins,
                total_return: ps.total_return,
                sharpe: ps.sharpe,
                success_score: ps.success_score,
            },
        );
    }

    println!("Total trades: {}, Winning: {}, Losing: {}, Win rate: {:.2}%", total_trades, winning_trades, losing_trades, win_rate * 100.0);
    Ok(EngineResult {
        total_return: engine.equity,
        sharpe_ratio,
        total_trades,
        winning_trades,
        losing_trades,
        win_rate,
        max_drawdown: max_dd,
        // Final value approximation: current cash + capital still tied in open positions (at cost basis).
        // NOTE: Unrealized PnL on open positions is not marked-to-market here; could be added later.
        final_value: engine.cash + engine.invested_capital,
        max_concurrent_positions: engine.max_concurrent,
        pair_performance,
        trade_log: engine.trade_log,
    })
}
