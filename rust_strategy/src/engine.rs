use crate::data::load_market_data;
use crate::params::Params;
use crate::stats::PairStats;
use ahash::AHasher;
use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

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
}

#[derive(Clone, PartialEq, Debug)]
enum PositionKind {
    LongSpread,
    ShortSpread,
}

const STARTING_CASH: f64 = 100_000.0;
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
    cash: f64,
    // Track today's volume per contract (if provided)
    daily_volume: HashMap<String, u32>,
    // If commodity_a_prefix == commodity_b_prefix we operate in single commodity mode
    // and generate spreads from distinct contracts within the same commodity.
    single_commodity: bool,
}

impl<'a> Engine<'a> {
    /// Create a new Engine with empty state.
    fn new(params: &'a Params) -> Self {
        let single = params.commodity_a_prefix == params.commodity_b_prefix;
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
            cash: STARTING_CASH,
            daily_volume: HashMap::new(),
            single_commodity: single,
        }
    }
}

fn close_trade(pos: &Position, cur_price: f64) -> (f64, f64) {
    // Returns (raw_gain, trade_ret) both scaled by position size.
    // raw_gain: directional exposure notionally closed out (signed) * size
    // trade_ret: realized PnL of the spread move * size
    let size_f = pos.size as f64;
    match pos.kind {
        PositionKind::LongSpread => (cur_price * size_f, (cur_price - pos.entry_spread) * size_f),
        PositionKind::ShortSpread => (-cur_price * size_f, (pos.entry_spread - cur_price) * size_f),
    }
}

impl<'a> Engine<'a> {
    fn cur_price(&self, pair: &(String, String)) -> Option<f64> {
        self.spread_histories
            .get(pair)
            .and_then(|hist| hist.back().copied())
    }

    // Gross exposure: sum of |spread| * size for all open positions using latest spread.
    fn gross_exposure(&self) -> f64 {
        self.active_positions
            .iter()
            .filter_map(|(pair, pos)| self.cur_price(pair).map(|p| p.abs() * pos.size as f64))
            .sum()
    }

    // Net exposure: directional exposure treating long spread as +spread and short spread as -spread.
    #[allow(dead_code)]
    fn net_exposure(&self) -> f64 {
        self.active_positions
            .iter()
            .filter_map(|(pair, pos)| {
                self.cur_price(pair).map(|p| match pos.kind {
                    PositionKind::LongSpread => p * pos.size as f64,
                    PositionKind::ShortSpread => -p * pos.size as f64,
                })
            })
            .sum()
    }

    // Common logic to finalize closing a position, updating equity, cash, stats, and logging.
    // reason: textual reason (e.g., "reversion", "expiry"). Optionally supply current z / last_spread.
    fn finalize_close(
        &mut self,
        pair: (String, String),
        pos: Position,
        reason: &str,
    ) -> Result<()> {
        let Some(cur_price) = self.cur_price(&pair) else {
            return Err(anyhow::anyhow!("No current price available"));
        };
        let (raw_gain, trade_ret) = close_trade(&pos, cur_price);
        self.cash += raw_gain;
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
            size: pos.size,
            entry_bar: pos.entry_bar,
            exit_bar: self.bar_count,
            ret: trade_ret,
            entry_spread: pos.entry_spread,
            exit_spread: cur_price,
            reason: reason.into(),
            trade_id: pos.trade_id,
        });
        Ok(())
    }

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
    }

    fn close_positions(&mut self) {
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, _pos) in self.active_positions.iter() {
            if let Some(hist) = self.spread_histories.get(pair) {
                if hist.len() >= self.params.lookback_zscore {
                    if let Some((z, _)) = self.calc_z(hist) {
                        if z.abs() < self.params.exit_z || z.abs() > 5.0 {
                            to_close.push(pair.clone());
                        }
                    }
                }
            }
        }
        for pair in to_close {
            // Flattened control flow using early continue to avoid deep nesting.
            let Some(pos) = self.active_positions.remove(&pair) else {
                continue;
            };
            let _ = self.finalize_close(pair, pos, "reversion");
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
        for (a, _oi_a) in a_contracts_today {
            for (b, _oi_b) in b_contracts_today {
                // In single commodity mode we form intra-commodity pairs only once: enforce lexical order
                // and skip identical contracts.
                if a >= b {
                    continue;
                }
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
                let (z, cur_price) = match self.calc_z(hist) {
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
                let kind = if z > 0.0 {
                    PositionKind::ShortSpread
                } else {
                    PositionKind::LongSpread
                };

                if cur_price == 0.0 {
                    continue;
                }
                // Exposure-based sizing: cap gross exposure at 3x current equity.
                let safe_exposure = 3.0 * self.equity - self.gross_exposure();
                let allocation = safe_exposure.min(self.equity * 0.60).min(self.cash);
                let mut size: u32 = (allocation / cur_price.abs()).floor() as u32;

                // Liquidity cap: limit notional size to 1% of daily volume of the less liquid leg (if volume available)
                if let (Some(v_a), Some(v_b)) = (self.daily_volume.get(a), self.daily_volume.get(b))
                {
                    let vol_cap = (*v_a).min(*v_b) as f64 * 0.01; // 1% of lesser volume
                    let vol_cap_u = vol_cap.floor() as u32;
                    if vol_cap_u > 0 {
                        size = size.min(vol_cap_u);
                    }
                }
                if size <= 0 {
                    continue;
                }
                let cost = size as f64 * cur_price * -z.signum();
                println!(
                    "ENTER candidate {:?} z={:.3} spread={:.3} size={}",
                    pair, z, cur_price, size
                );
                self.cash -= cost; // cost can be negative

                let mut hasher = AHasher::default();
                format!("{}_{}", a, b).hash(&mut hasher);
                let trade_id = hasher.finish();
                if self.params.debug {
                    eprintln!("ENTER {:?} {:?} z={:.3}", kind, pair, z);
                }
                self.active_positions.insert(
                    pair.clone(),
                    Position {
                        pair: pair.clone(),
                        kind,
                        entry_z: z,
                        entry_bar: self.bar_count,
                        entry_spread: cur_price,
                        size: size,
                        trade_id,
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
                let _ = self.finalize_close(pair, pos, "expiry");
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
    let md: crate::data::MarketData = load_market_data(path)?;
    if params.debug {
        eprintln!(
            "Loaded DataFrame: rows={}, cols={}",
            md.df.height(),
            md.df.width()
        );
    }
    println!("Unique trading days: {}", md.trading_days.len());

    // Pre-group day -> rows indices to avoid filtering repeatedly (simplified)
    // (Could optimize with polars groupby but keep simple for clarity.)
    // We'll collect row indices by date.

    let mut date_indices: HashMap<NaiveDate, Vec<usize>> = HashMap::new();
    let contract_series = md.df.column("Contract")?.str()?;
    let close_series = md.df.column("Close")?.f64()?;
    let oi_series = md.df.column("OI").ok().and_then(|s| s.f64().ok());
    let vol_series = md.df.column("Volume").ok().and_then(|s| s.u32().ok());
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
        if engine.cash <= 0.0 {
            break;
        }
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
            if let Some(vol) = vol_series.and_then(|col| col.get(i)) {
                if let Some(c) = contract {
                    // Use per-row (per contract) volume; latest value wins for the day.
                    engine.daily_volume.insert(c.to_string(), vol);
                }
            }
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
                a_contracts_today.iter().map(|(c, _)| c).collect::<Vec<_>>(),
                b_contracts_today.iter().map(|(c, _)| c).collect::<Vec<_>>()
            );
        }

        // Update spreads
        if engine.single_commodity {
            // For single commodity mode, treat the collected a_contracts_today as the universe.
            // We'll compute spreads only for ordered pairs (i < j) to avoid duplicates and self-pairs.
            for i in 0..a_contracts_today.len() {
                let (ref a, _oi_a) = a_contracts_today[i];
                if let Some(&a_p) = a_prices.get(a) {
                    for j in (i + 1)..a_contracts_today.len() {
                        let (ref b, _oi_b) = a_contracts_today[j];
                        if let Some(&b_p) = a_prices.get(b) {
                            // same map
                            engine.push_spread((a.clone(), b.clone()), a_p - b_p);
                        }
                    }
                }
            }
        } else {
            for (a, _oi_a) in &a_contracts_today {
                if let Some(&a_p) = a_prices.get(a) {
                    for (b, _oi_b) in &b_contracts_today {
                        if let Some(&b_p) = b_prices.get(b) {
                            engine.push_spread((a.clone(), b.clone()), a_p - b_p);
                        }
                    }
                }
            }
        }

        // First close positions that are near expiry
        engine.close_expiring_positions();
        // Then normal mean reversion closures
        engine.close_positions();
        if engine.single_commodity {
            // Reuse same list for both sides since it's the same commodity universe.
            engine.try_enter_positions(&a_contracts_today, &a_contracts_today);
        } else {
            engine.try_enter_positions(&a_contracts_today, &b_contracts_today);
        }
        // Update equity based on current positions and cash
        let mut position_value = 0.0;
        for (pair, pos) in &engine.active_positions {
            if let Some(cur_price) = engine.cur_price(pair) {
                position_value += cur_price * pos.size as f64;
            }
        }
        engine.equity = engine.cash + position_value;
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

    println!(
        "Total trades: {}, Winning: {}, Losing: {}, Win rate: {:.2}%",
        total_trades,
        winning_trades,
        losing_trades,
        win_rate * 100.0
    );

    // Sort trade log so the largest magnitude winners/losers appear first (big losses & big gains).
    engine.trade_log.sort_by(|a, b| {
        b.ret
            .abs()
            .partial_cmp(&a.ret.abs())
            .unwrap_or(Ordering::Equal)
    });

    Ok(EngineResult {
        final_value: engine.equity,
        sharpe_ratio,
        total_trades,
        winning_trades,
        losing_trades,
        win_rate,
        max_drawdown: max_dd,
        // Final value approximation: current cash + capital still tied in open positions (at cost basis).
        // NOTE: Unrealized PnL on open positions is not marked-to-market here; could be added later.
        total_return: engine.equity - STARTING_CASH,
        max_concurrent_positions: engine.max_concurrent,
        pair_performance,
        trade_log: engine.trade_log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_pos(entry_spread: f64, kind: PositionKind) -> Position {
        Position {
            pair: ("a".into(), "b".into()),
            kind,
            entry_z: 0.0,
            entry_bar: 0,
            entry_spread,
            size: 1,
            trade_id: 1,
        }
    }

    #[test]
    fn percent_moved_long_negative_entry_to_positive_exit() {
        // Case from user report: entry -210 -> exit 105, long spread raw move = 315; size=1 => PnL 315
        let pos = mk_pos(-210.0, PositionKind::LongSpread);
        let (raw_gain, trade_ret) = close_trade(&pos, 105.0);
        // For a long spread we store raw_gain = cur_price * size (105)
        assert!(
            (raw_gain - 105.0).abs() < 1e-9,
            "expected raw_gain 105 got {}",
            raw_gain
        );
        assert!(
            (trade_ret - 315.0).abs() < 1e-9,
            "expected 315 trade_ret got {}",
            trade_ret
        );
    }

    #[test]
    fn percent_moved_short_positive_move_down() {
        // Short spread: entry 500 -> exit 350, raw directional move = 150 (profit); size=1 => PnL 150
        let pos = mk_pos(500.0, PositionKind::ShortSpread);
        let (raw_gain, trade_ret) = close_trade(&pos, 350.0);
        // For a short spread raw_gain = -cur_price * size (-350)
        assert!(
            (raw_gain + 350.0).abs() < 1e-9,
            "expected raw_gain -350 got {}",
            raw_gain
        );
        assert!(
            (trade_ret - 150.0).abs() < 1e-9,
            "expected 150 trade_ret got {}",
            trade_ret
        );
    }

    #[test]
    fn percent_moved_zero_entry_protected() {
        let pos = mk_pos(0.0, PositionKind::LongSpread);
        let (raw_gain, trade_ret) = close_trade(&pos, 10.0);
        assert!(
            (raw_gain - 10.0).abs() < 1e-9,
            "expected raw_gain 10 got {}",
            raw_gain
        );
        // entry spread zero -> move = 10 - 0 = 10; still valid
        assert_eq!(trade_ret, 10.0);
    }

    #[test]
    fn trade_return_long_negative_spread() {
        // Entry spread -210, exit +105 -> raw move = 315, size=1 => return 315
        let pos = mk_pos(-210.0, PositionKind::LongSpread);
        let (raw_gain, trade_ret) = close_trade(&pos, 105.0);
        assert!(
            (raw_gain - 105.0).abs() < 1e-9,
            "expected raw_gain 105 got {}",
            raw_gain
        );
        assert!(
            (trade_ret - 315.0).abs() < 1e-6,
            "expected 315 got {}",
            trade_ret
        );
    }

    #[test]
    fn trade_return_scales_with_size() {
        // Same spread move as previous test but with size=4
        let mut pos = mk_pos(-210.0, PositionKind::LongSpread);
        pos.size = 4;
        let (raw_gain, trade_ret) = close_trade(&pos, 105.0);
        assert!(
            (raw_gain - 4.0 * 105.0).abs() < 1e-9,
            "expected raw_gain {} got {}",
            4.0 * 105.0,
            raw_gain
        );
        assert!(
            (trade_ret - 4.0 * 315.0).abs() < 1e-6,
            "expected {} got {}",
            4.0 * 315.0,
            trade_ret
        );
    }
}
