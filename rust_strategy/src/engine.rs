use crate::data::{load_market_data, MarketData};
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

// Helper: compute mean, std, z of last value; returns (z, last_spread)
fn calc_z(hist: &VecDeque<f64>) -> Option<(f64, f64)> {
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
    let last = *hist.back().unwrap();
    Some(((last - mean) / std, last))
}

fn push_spread(
    spread_histories: &mut HashMap<(String, String), VecDeque<f64>>,
    pair: (String, String),
    spread: f64,
    lookback: usize,
    debug: bool,
) {
    let entry = spread_histories
        .entry(pair.clone())
        .or_insert_with(|| VecDeque::with_capacity(lookback));
    if entry.len() == lookback {
        entry.pop_front();
    }
    entry.push_back(spread);
    if debug && entry.len() == lookback {
        eprintln!(
            "Pair {:?} reached lookback with last spread {:.4}",
            pair, spread
        );
    }
}

fn close_positions(
    active_positions: &mut HashMap<(String, String), Position>,
    spread_histories: &HashMap<(String, String), VecDeque<f64>>,
    pair_stats: &mut HashMap<(String, String), PairStats>,
    trade_log: &mut Vec<TradeLogEntry>,
    bar_count: usize,
    params: &Params,
    equity: &mut f64,
) {
    let mut to_close: Vec<(String, String)> = Vec::new();
    for (pair, _pos) in active_positions.iter() {
        if let Some(hist) = spread_histories.get(pair) {
            if hist.len() >= params.lookback_zscore {
                if let Some((z, _last)) = calc_z(hist) {
                    if z.abs() < params.exit_z || z.abs() > 5.0 {
                        to_close.push(pair.clone());
                    }
                }
            }
        }
    }
    for pair in to_close {
        let pos = match active_positions.remove(&pair) {
            Some(p) => p,
            None => continue,
        };

        let hist = match spread_histories.get(&pair) {
            Some(h) => h,
            None => continue,
        };

        let (z, _last_spread) = match calc_z(hist) {
            Some(val) => val,
            None => continue,
        };

        let mut trade_ret = z - pos.entry_z;
        if pos.kind == PositionKind::ShortSpread {
            trade_ret = -trade_ret;
        }

        *equity += trade_ret;
        let stats = pair_stats
            .entry(pair.clone())
            .or_insert_with(|| PairStats::new(params.lookback_performance));
        stats.record(trade_ret, bar_count);

        trade_log.push(TradeLogEntry {
            pair: format!("{}/{}", pair.0, pair.1),
            kind: match pos.kind {
                PositionKind::LongSpread => "long_spread".into(),
                PositionKind::ShortSpread => "short_spread".into(),
            },
            entry_bar: pos.entry_bar,
            exit_bar: bar_count,
            entry_z: pos.entry_z,
            exit_z: z,
            ret: trade_ret,
            reason: "reversion".into(),
            trade_id: pos.trade_id,
        });
    }
}

fn try_enter_positions(
    active_positions: &mut HashMap<(String, String), Position>,
    spread_histories: &HashMap<(String, String), VecDeque<f64>>,
    cu_contracts_today: &[(String, f64)],
    fu_contracts_today: &[(String, f64)],
    params: &Params,
    bar_count: usize,
    debug: bool,
) {
    if active_positions.len() >= params.max_active_pairs {
        return;
    }
    for (cu, _oi_cu) in cu_contracts_today {
        for (fu, _oi_fu) in fu_contracts_today {
            let pair = (cu.clone(), fu.clone());
            if active_positions.len() >= params.max_active_pairs { return; }
            if active_positions.contains_key(&pair) { continue; }

            let hist = match spread_histories.get(&pair) { Some(h) => h, None => continue };
            if hist.len() < params.lookback_zscore { continue; }
            let (z, last_spread) = match calc_z(hist) { Some(v) => v, None => continue };
            if debug && z.abs() > params.entry_z * 0.5 {
                eprintln!("Candidate {:?} z={:.3} (threshold {})", pair, z, params.entry_z);
            }
            if z.abs() <= params.entry_z { continue; }

            let mut hasher = AHasher::default();
            format!("{}_{}", cu, fu).hash(&mut hasher);
            let trade_id = hasher.finish();
            let kind = if z > 0.0 { PositionKind::ShortSpread } else { PositionKind::LongSpread };
            if debug { eprintln!("ENTER {:?} {:?} z={:.3}", kind, pair, z); }
            active_positions.insert(pair.clone(), Position { pair: pair.clone(), kind, entry_z: z, entry_bar: bar_count, entry_spread: last_spread, size: ((z.abs()*2.0).floor() as u32).max(1).min(10), trade_id });
        }
    }
}

// Helper to process a single dataframe row, inserting prices and optional OI ranked candidates.
// Returns None if required values (contract/close) are missing so caller can use `let _ = process_row(...);`.
fn process_row(
    contract: Option<&str>,
    close: Option<f64>,
    oi: Option<f64>,
    cu_prices: &mut HashMap<String, f64>,
    fu_prices: &mut HashMap<String, f64>,
    cu_contracts_today: &mut Vec<(String, f64)>,
    fu_contracts_today: &mut Vec<(String, f64)>,
) -> Option<()> {
    let contract = contract?;
    let close = close?;

    if contract.starts_with("cu") {
        cu_prices.insert(contract.to_string(), close);
    } else if contract.starts_with("fu") {
        fu_prices.insert(contract.to_string(), close);
    } else {
        return Some(()); // ignore other prefixes
    }

    if let Some(oi_val) = oi {
        if contract.starts_with("cu") {
            cu_contracts_today.push((contract.to_string(), oi_val));
        } else if contract.starts_with("fu") {
            fu_contracts_today.push((contract.to_string(), oi_val));
        }
    }
    Some(())
}

pub fn run_engine(path: &str, params: &Params) -> Result<EngineResult> {
    let md = load_market_data(path)?;
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

    let mut spread_histories: HashMap<(String, String), VecDeque<f64>> = HashMap::new();
    let mut active_positions: HashMap<(String, String), Position> = HashMap::new();
    let mut pair_stats: HashMap<(String, String), PairStats> = HashMap::new();
    let mut trade_log: Vec<TradeLogEntry> = Vec::new();

    let mut bar_count: usize = 0;
    let mut equity: f64 = 0.0; // relative equity change
    let mut equity_curve: Vec<f64> = Vec::new();

    for day in &md.trading_days {
        bar_count += 1;
        let indices = if let Some(v) = date_indices.get(day) {
            v
        } else {
            continue;
        }; // no data -> skip
        if params.debug && bar_count <= 5 {
            eprintln!("Day {:?}: indices={}", day, indices.len());
        }

        // Collect today's prices and OI
        let mut cu_prices: HashMap<String, f64> = HashMap::new();
        let mut fu_prices: HashMap<String, f64> = HashMap::new();
        let mut cu_contracts_today: Vec<(String, f64)> = Vec::new();
        let mut fu_contracts_today: Vec<(String, f64)> = Vec::new();

        for &i in indices {
            let contract = contract_series.get(i);
            let close = close_series.get(i);
            let oi_val = oi_series.and_then(|col| col.get(i));
            let _ = process_row(
                contract,
                close,
                oi_val,
                &mut cu_prices,
                &mut fu_prices,
                &mut cu_contracts_today,
                &mut fu_contracts_today,
            );
        }

        // Sort by OI and take top 3 each (if OI available). If OI absent, fallback to all seen contracts today.
        if !cu_contracts_today.is_empty() {
            cu_contracts_today
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            cu_contracts_today.truncate(3);
        } else {
            // fallback: all cu_prices keys
            for k in cu_prices.keys() {
                cu_contracts_today.push((k.clone(), 0.0));
            }
        }
        if !fu_contracts_today.is_empty() {
            fu_contracts_today
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            fu_contracts_today.truncate(3);
        } else {
            for k in fu_prices.keys() {
                fu_contracts_today.push((k.clone(), 0.0));
            }
        }
        if params.debug && bar_count <= 5 {
            eprintln!(
                "Top cu today: {:?}; Top fu today: {:?}",
                cu_contracts_today
                    .iter()
                    .map(|(c, _)| c)
                    .collect::<Vec<_>>(),
                fu_contracts_today
                    .iter()
                    .map(|(c, _)| c)
                    .collect::<Vec<_>>()
            );
        }

        // Update spreads
        for (cu, _oi_cu) in &cu_contracts_today {
            if let Some(&cu_p) = cu_prices.get(cu) {
                for (fu, _oi_fu) in &fu_contracts_today {
                    if let Some(&fu_p) = fu_prices.get(fu) {
                        push_spread(
                            &mut spread_histories,
                            (cu.clone(), fu.clone()),
                            cu_p - fu_p,
                            params.lookback_zscore,
                            params.debug,
                        );
                    }
                }
            }
        }

        close_positions(
            &mut active_positions,
            &spread_histories,
            &mut pair_stats,
            &mut trade_log,
            bar_count,
            params,
            &mut equity,
        );

        try_enter_positions(
            &mut active_positions,
            &spread_histories,
            &cu_contracts_today,
            &fu_contracts_today,
            params,
            bar_count,
            params.debug,
        );

        equity_curve.push(equity);
    }

    // Aggregate statistics
    let total_trades = trade_log.len();
    let winning_trades = trade_log.iter().filter(|t| t.ret > 0.0).count();
    let losing_trades = total_trades - winning_trades;
    let win_rate = if total_trades > 0 {
        winning_trades as f64 / total_trades as f64
    } else {
        0.0
    };

    // Sharpe on equity changes per bar
    let sharpe_ratio = if equity_curve.len() >= 5 {
        let mean = equity_curve.iter().copied().sum::<f64>() / equity_curve.len() as f64;
        let var = equity_curve
            .iter()
            .map(|v| {
                let d = v - mean;
                d * d
            })
            .sum::<f64>()
            / equity_curve.len() as f64;
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
    for v in &equity_curve {
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
    for ((c1, c2), ps) in pair_stats.into_iter() {
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

    Ok(EngineResult {
        total_return: equity,
        sharpe_ratio,
        total_trades,
        winning_trades,
        losing_trades,
        win_rate,
        max_drawdown: max_dd,
        final_value: 100000.0 * (1.0 + equity / 100.0),
        pair_performance,
        trade_log,
    })
}
