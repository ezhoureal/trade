use std::collections::{HashMap, VecDeque};

use crate::engine::ContractsToday;


#[derive(Clone, Debug)]
pub struct Position {
    pair: (String, String),
    kind: PositionKind,
    entry_z: f64,
    entry_bar: usize,
    entry_spread: f64,
    size: u32,
    trade_id: u64,
}


#[derive(Clone, PartialEq, Debug)]
pub enum PositionKind {
    LongSpread,
    ShortSpread,
}
pub struct PairStrategy {
    spread_histories: HashMap<(String, String), VecDeque<f64>>,
    active_positions: HashMap<(String, String), Position>,
    cash: f32,
    equity: f32,

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

impl PairStrategy {
    pub fn trade(bar: u32, a: ContractsToday, b: ContractsToday) {
    }
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

                // Exposure-based sizing: cap gross exposure at 3x current equity.
                let safe_exposure = 3.0 * self.equity - self.gross_exposure();
                let allocation = safe_exposure.min(self.equity * 0.50).min(self.cash);
                let mut size: u32 = (allocation / cur_price.abs()).floor() as u32;

                // Liquidity cap: limit notional size to 1% of daily volume of the less liquid leg (if volume available)
                if let (Some(v_a), Some(v_b)) = (self.daily_volume.get(a), self.daily_volume.get(b))
                {
                    let vol_cap = (*v_a).min(*v_b) as f32 * 0.01; // 1% of lesser volume
                    let vol_cap_u = vol_cap.floor() as u32;
                    size = size.min(vol_cap_u);
                } else {
                    println!("WARNING: no volume data");
                }
                if size <= 0 {
                    continue;
                }
                let cost = size as f64 * cur_price * -z.signum();
                if self.params.debug {
                    println!(
                        "ENTER candidate {:?} z={:.3} spread={:.3} size={}",
                        pair, z, cur_price, size
                    );
                }
                self.cash -= cost; // cost can be negative

                let mut hasher = AHasher::default();
                format!("{}_{}", a, b).hash(&mut hasher);
                let trade_id = hasher.finish();
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

    fn stop_loss(&mut self) {
        const LOSS_RATIO_THRESHOLD: f64 = 0.2;
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, pos) in self.active_positions.iter() {
            let cur_price = self.cur_price(pair).unwrap();
            let loss = match pos.kind {
                PositionKind::LongSpread => pos.entry_spread - cur_price,
                PositionKind::ShortSpread => cur_price - pos.entry_spread,
            };
            if loss / pos.entry_spread > LOSS_RATIO_THRESHOLD {
                to_close.push(pair.clone());
            }
        }
        for pair in to_close {
            if let Some(pos) = self.active_positions.remove(&pair) {
                let _ = self.finalize_close(pair, pos, "stop_loss");
            }
        }
    }
}