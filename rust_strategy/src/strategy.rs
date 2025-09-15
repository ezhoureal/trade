use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

use crate::{
    engine::{Broker, ContractsToday, PositionKind},
    params::Params,
};
#[derive(Debug, Serialize)]
pub struct TradeLogEntry {
    pub pair: String,
    pub kind: String,
    pub entry_bar: u32,
    pub exit_bar: u32,
    pub ret: f32,
    pub size: u32,
    pub entry_spread: f32,
    pub exit_spread: f32,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct PairPosition {
    pair: (String, String),
    kind: PositionKind,
    entry_z: f32,
    entry_bar: u32,
    entry_spread: f32,
    size: u32,
}

pub struct PairStrategy<'a> {
    params: &'a Params,
    trade_log: Vec<TradeLogEntry>,
    spread_histories: HashMap<(String, String), VecDeque<f32>>,
    active_positions: HashMap<(String, String), PairPosition>,
    contract_expiry: HashMap<String, u32>,
    bar_count: u32,
    max_concurrent: usize,
}

impl<'a> PairStrategy<'a> {
    pub fn new(params: &'a Params, contract_expiry: HashMap<String, u32>) -> Self {
        PairStrategy {
            params,
            spread_histories: HashMap::new(),
            active_positions: HashMap::new(),
            contract_expiry,
            bar_count: 0,
            trade_log: Vec::new(),
            max_concurrent: 0,
        }
    }

    pub fn trade(
        &mut self,
        bar: u32,
        a: ContractsToday,
        b: ContractsToday,
        broker: &mut dyn Broker,
    ) -> Result<()> {
        self.bar_count = bar;
        // Build / update spread histories for all observed pairs today.
        for contr_a in a.iter() {
            for contr_b in b.iter() {
                let spread = contr_a.price - contr_b.price;
                self.push_spread((contr_a.name.clone(), contr_b.name.clone()), spread);
            }
        }
        // Housekeeping / risk management
        self.close_expiring_positions(broker);
        self.close_reverted(broker);
        // self.try_enter_positions(&a, &b, broker)?; // enable when entry logic desired
        self.stop_loss(broker);
        Ok(())
    }

    fn cur_price(&self, pair: &(String, String)) -> Option<f32> {
        self.spread_histories
            .get(pair)
            .and_then(|hist| hist.back().copied())
    }

    // Gross exposure: sum of |spread| * size for all open positions using latest spread.
    fn gross_exposure(&self) -> f32 {
        self.active_positions
            .iter()
            .filter_map(|(pair, pos)| self.cur_price(pair).map(|p| p.abs() * pos.size as f32))
            .sum()
    }

    fn close(
        &mut self,
        pair: (String, String),
        pos: PairPosition,
        reason: &str,
        broker: &mut dyn Broker,
    ) -> Result<()> {
        match pos.kind {
            PositionKind::Long => {
                broker.sell(pair.0.as_str(), pos.size);
                broker.buy(pair.1.as_str(), pos.size);
            }
            PositionKind::Short => {
                broker.buy(pair.0.as_str(), pos.size);
                broker.sell(pair.1.as_str(), pos.size);
            }
        }

        let cur_price = self
            .cur_price(&pair)
            .ok_or_else(|| anyhow::anyhow!("No current price for pair {}/{}", pair.0, pair.1))?;
        let trade_ret = match pos.kind {
            PositionKind::Long => (cur_price - pos.entry_spread) / pos.entry_spread,
            PositionKind::Short => (pos.entry_spread - cur_price) / pos.entry_spread,
        };
        self.trade_log.push(TradeLogEntry {
            pair: format!("{}/{}", pair.0, pair.1),
            kind: match pos.kind {
                PositionKind::Long => "long".into(),
                PositionKind::Short => "short".into(),
            },
            size: pos.size,
            entry_bar: pos.entry_bar,
            exit_bar: self.bar_count,
            ret: trade_ret,
            entry_spread: pos.entry_spread,
            exit_spread: self.cur_price(&pair).unwrap(),
            reason: reason.into(),
        });
        Ok(())
    }

    // compute mean, std, z of last value; returns (z, last_spread)
    fn calc_z(&self, hist: &VecDeque<f32>) -> Option<(f32, f32)> {
        if hist.is_empty() {
            return None;
        }
        let len = hist.len();
        let mean = hist.iter().copied().sum::<f32>() / len as f32;
        let var = hist
            .iter()
            .map(|v| {
                let d = v - mean;
                d * d
            })
            .sum::<f32>()
            / len as f32;
        let std = var.sqrt();
        if std == 0.0 {
            return None;
        }
        let last = *hist.back()?;
        Some(((last - mean) / std, last))
    }

    fn push_spread(&mut self, pair: (String, String), spread: f32) {
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

    fn close_reverted(&mut self, broker: &mut dyn Broker) {
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
            if let Err(e) = self.close(pair, pos, "reversion", broker) {
                if self.params.debug {
                    eprintln!("close_reverted error: {e}");
                }
            }
        }
    }

    fn enter(
        &mut self,
        pair: (String, String),
        kind: PositionKind,
        z: f32,
        size: u32,
        broker: &mut dyn Broker,
    ) -> Option<()> {
        match kind {
            PositionKind::Long => {
                broker.buy(pair.0.as_str(), size);
                broker.sell(pair.1.as_str(), size);
            }
            PositionKind::Short => {
                broker.sell(pair.0.as_str(), size);
                broker.buy(pair.1.as_str(), size);
            }
        }

        self.active_positions.insert(
            pair.clone(),
            PairPosition {
                pair: pair.clone(),
                kind,
                entry_z: z,
                entry_bar: self.bar_count,
                entry_spread: self.cur_price(&pair)?,
                size: size,
            },
        );
        let cur_len = self.active_positions.len();
        if cur_len > self.max_concurrent {
            self.max_concurrent = cur_len;
        }
        Some(())
    }

    fn is_near_expiry(&self, contract: &str) -> bool {
        const EXPIRY_BUFFER: u32 = 7;
        let cur_day = self.bar_count;
        self.contract_expiry
            .get(contract)
            .map(|last| *last <= cur_day + EXPIRY_BUFFER)
            .unwrap_or(false)
    }

    fn try_enter_positions(
        &mut self,
        commodity_a: &ContractsToday,
        commodity_b: &ContractsToday,
        broker: &mut dyn Broker,
    ) -> Option<()> {
        for contr_a in commodity_a.iter() {
            for contr_b in commodity_b.iter() {
                if contr_a.name >= contr_b.name {
                    continue; // avoid duplicate pairs
                }
                let pair = (contr_a.name.clone(), contr_b.name.clone());
                if self.active_positions.contains_key(&pair) {
                    continue;
                }
                if self.is_near_expiry(&contr_a.name) || self.is_near_expiry(&contr_b.name) {
                    continue;
                }
                let hist = self.spread_histories.get(&pair)?;
                if hist.len() < self.params.lookback_zscore {
                    continue;
                }
                let (z, cur_price) = self.calc_z(hist)?;

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
                    PositionKind::Short
                } else {
                    PositionKind::Long
                };

                let status = broker.get_status();
                if status.cash <= 0.0 {
                    continue;
                }
                // Exposure-based sizing: cap gross exposure at 3x current equity.
                let safe_exposure = 3.0 * status.equity - self.gross_exposure();
                let allocation = safe_exposure.min(status.equity * 0.50).min(status.cash);
                let mut size: u32 = (allocation / cur_price.abs()).floor() as u32;

                let vol_cap = contr_a.volume.min(contr_b.volume) as f32 * 0.01; // 1% of lesser volume
                let vol_cap_u = vol_cap.floor() as u32;
                size = size.min(vol_cap_u);

                if size <= 0 {
                    continue;
                }
                self.enter(pair, kind, z, size, broker);
            }
        }
        Some(())
    }

    fn close_expiring_positions(&mut self, broker: &mut dyn Broker) -> Option<()> {
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, _pos) in self.active_positions.iter() {
            // Force close if either leg is within expiry_close_days of its last observed day
            let (ref c1, ref c2) = pair;
            let expiry_window = self.params.expiry_close_days;
            let cur_day = self.bar_count; // bar_count is 1-based per loop increment
            let needs_close = self
                .contract_expiry
                .get(c1)
                .map(|d| *d <= cur_day + expiry_window)
                .unwrap_or(false)
                || self
                    .contract_expiry
                    .get(c2)
                    .map(|d| *d <= cur_day + expiry_window)
                    .unwrap_or(false);
            if needs_close {
                to_close.push(pair.clone());
            }
        }
        for pair in to_close {
            let pos = self.active_positions.remove(&pair)?;
            if let Err(e) = self.close(pair, pos, "expiry", broker) {
                if self.params.debug {
                    eprintln!("close_expiring error: {e}");
                }
            }
        }
        Some(())
    }

    fn stop_loss(&mut self, broker: &mut dyn Broker) {
        const LOSS_RATIO_THRESHOLD: f32 = 0.2;
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, pos) in self.active_positions.iter() {
            let cur_price = self.cur_price(pair).unwrap();
            let loss = match pos.kind {
                PositionKind::Long => pos.entry_spread - cur_price,
                PositionKind::Short => cur_price - pos.entry_spread,
            };
            if loss / pos.entry_spread > LOSS_RATIO_THRESHOLD {
                to_close.push(pair.clone());
            }
        }
        for pair in to_close {
            if let Some(pos) = self.active_positions.remove(&pair) {
                if let Err(e) = self.close(pair, pos, "stop_loss", broker) {
                    if self.params.debug {
                        eprintln!("stop_loss close error: {e}");
                    }
                }
            }
        }
    }
}
