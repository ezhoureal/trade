use anyhow::Result;
use polars::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};

use crate::{
    engine::{Broker, ContractData, ContractsToday, PositionKind},
    params::Params,
};
#[derive(Debug, Serialize)]
pub struct TradeLogEntry {
    pub pair: String,
    pub kind: String,
    pub entry_bar: u32,
    pub exit_bar: u32,
    pub entry_z: f32,
    pub ret: f32,
    pub ret_pct: f32,
    pub size: u32,
    pub entry_spread: f32,
    pub exit_spread: f32,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct PairPosition {
    kind: PositionKind,
    entry_z: f32,
    entry_bar: u32,
    entry_spread: f32,
    gross_notional: f32, // exposure at entry (for PnL % calculation)
    size: u32,
}

pub struct PairStrategy {
    params: Params,
    trade_log: Vec<TradeLogEntry>,
    spread_histories: HashMap<(String, String), VecDeque<f32>>,
    active_positions: HashMap<(String, String), PairPosition>,
    contract_expiry: HashMap<String, u32>,
    bar_count: u32,
}

impl PairStrategy {
    pub fn new(params: Params, contract_expiry: HashMap<String, u32>) -> Self {
        PairStrategy {
            params,
            spread_histories: HashMap::new(),
            active_positions: HashMap::new(),
            contract_expiry,
            bar_count: 0,
            trade_log: Vec::new(),
        }
    }

    #[cfg(feature="live")]
    pub fn new_live(params: Params, df: LazyFrame) -> Self {
        let mut strategy = PairStrategy {
            params,
            spread_histories: HashMap::new(),
            active_positions: HashMap::new(),
            contract_expiry: HashMap::new(),
            bar_count: 0,
            trade_log: Vec::new(),
        };
        strategy
            .load_spread_history(df)
            .expect("Failed to load spread history");
        strategy
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
        self.close_expiring_positions(broker);
        self.close_reverted(broker);

        self.try_enter_positions(a, b, broker)
            .ok_or(anyhow::anyhow!("failed"))?;

        self.stop_loss(broker);
        Ok(())
    }

    pub fn move_log(mut self) -> Vec<TradeLogEntry> {
        self.trade_log.sort_by(|a, b| {
            b.ret
                .abs()
                .partial_cmp(&a.ret.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let return_sum = self.trade_log.iter().map(|e| e.ret).sum::<f32>();
        println!("Total return from all trades: {:.2}", return_sum);
        self.trade_log
    }

    fn cur_price(&self, pair: &(String, String)) -> Option<f32> {
        self.spread_histories
            .get(pair)
            .and_then(|hist| hist.back().copied())
    }

    fn close(
        &mut self,
        pair: (String, String),
        pos: PairPosition,
        reason: &str,
        broker: &mut dyn Broker,
    ) -> Option<()> {
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

        let cur_price = self.cur_price(&pair)?;
        let pnl = match pos.kind {
            PositionKind::Long => cur_price - pos.entry_spread,
            PositionKind::Short => pos.entry_spread - cur_price,
        } * pos.size as f32
            - self.params.transaction_cost_pct * pos.gross_notional * 2.0;
        self.trade_log.push(TradeLogEntry {
            pair: format!("{}/{}", pair.0, pair.1),
            kind: match pos.kind {
                PositionKind::Long => "long".into(),
                PositionKind::Short => "short".into(),
            },
            size: pos.size,
            entry_bar: pos.entry_bar,
            exit_bar: self.bar_count,
            entry_z: pos.entry_z,
            ret: pnl,
            ret_pct: pnl / pos.gross_notional,
            entry_spread: pos.entry_spread,
            exit_spread: cur_price,
            reason: reason.into(),
        });
        Some(())
    }

    // compute mean, std, z of last value; returns (z, last_spread)
    fn calc_z(&self, hist: &VecDeque<f32>) -> Option<f32> {
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
        Some((last - mean) / std)
    }

    fn push_spread(&mut self, pair: (String, String), spread: f32) {
        let lookback = self.params.lookback_zscore;
        let entry = self
            .spread_histories
            .entry(pair)
            .or_insert_with(|| VecDeque::with_capacity(lookback));
        if entry.len() == lookback {
            entry.pop_front();
        }
        entry.push_back(spread);
    }

    #[cfg(feature="live")]
    pub fn pop_spread(&mut self) {
        for (_, history) in self.spread_histories.iter_mut() {
            history.pop_back();
        }
    }

    #[cfg(feature="live")]
    fn load_spread_history(&mut self, df: LazyFrame) -> PolarsResult<()> {
        let sort_opts = SortMultipleOptions {
            descending: vec![false],
            maintain_order: false,
            nulls_last: vec![false],
            multithreaded: true,
            limit: None,
        };
        let sorted = df.sort_by_exprs(vec![col("Date")], sort_opts).collect()?;
        let same_prefix = self.params.commodity_a_prefix == self.params.commodity_b_prefix;
        let contracts_s = sorted.column("Contract")?.str()?;
        let prices_f = sorted.column("Price")?.f64()?;
        let date_col = sorted.column("Date")?;
        let mut last_date: Option<AnyValue> = None;
        let mut a_contracts: Vec<(String, f32)> = Vec::new();
        let mut b_contracts: Vec<(String, f32)> = Vec::new();
        for row in 0..sorted.height() {
            let cur_date = date_col.get(row)?;
            if let Some(ref ld) = last_date {
                if &cur_date != ld {
                    self.flush_day(&a_contracts, &b_contracts, same_prefix);
                    a_contracts.clear();
                    b_contracts.clear();
                }
            }
            last_date = Some(cur_date);
            if let Some(contract) = contracts_s.get(row) {
                if let Some(price_v) = prices_f.get(row) {
                    let price = price_v as f32;
                    let c_l = contract.to_lowercase();
                    if same_prefix {
                        if c_l.starts_with(&self.params.commodity_a_prefix) {
                            a_contracts.push((contract.to_string(), price));
                        }
                    } else {
                        if c_l.starts_with(&self.params.commodity_a_prefix) {
                            a_contracts.push((contract.to_string(), price));
                        } else if c_l.starts_with(&self.params.commodity_b_prefix) {
                            b_contracts.push((contract.to_string(), price));
                        }
                    }
                }
            }
        }
        self.flush_day(&a_contracts, &b_contracts, same_prefix);
        Ok(())
    }

    fn flush_day(
        &mut self,
        a_contracts: &Vec<(String, f32)>,
        b_contracts: &Vec<(String, f32)>,
        same_prefix: bool,
    ) {
        if same_prefix {
            for i in 0..a_contracts.len() {
                for j in (i + 1)..a_contracts.len() {
                    let (ref name_i, price_i) = a_contracts[i];
                    let (ref name_j, price_j) = a_contracts[j];
                    let spread = price_i - price_j;
                    self.push_spread((name_i.clone(), name_j.clone()), spread);
                }
            }
        } else {
            for (name_a, price_a) in a_contracts.iter() {
                for (name_b, price_b) in b_contracts.iter() {
                    let spread = *price_a - *price_b;
                    self.push_spread((name_a.clone(), name_b.clone()), spread);
                }
            }
        }
    }

    fn close_reverted(&mut self, broker: &mut dyn Broker) -> Option<()> {
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, _pos) in self.active_positions.iter() {
            let hist = self.spread_histories.get(pair)?;
            if hist.len() >= self.params.lookback_zscore {
                let z = self.calc_z(hist)?;
                if z.abs() <= self.params.exit_z || z.abs() > self.params.entry_z * 2.0 {
                    to_close.push(pair.clone());
                }
            }
        }
        for pair in to_close {
            let pos = self.active_positions.remove(&pair)?;
            self.close(pair, pos, "reversion", broker)?;
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

    fn try_enter(
        &mut self,
        contr_a: &ContractData,
        contr_b: &ContractData,
        broker: &mut dyn Broker,
    ) -> Option<()> {
        if contr_a.name >= contr_b.name {
            return None; // avoid duplicate pairs
        }
        let pair = (contr_a.name.clone(), contr_b.name.clone());
        if self.active_positions.contains_key(&pair) {
            return None;
        }
        if self.is_near_expiry(&contr_a.name) || self.is_near_expiry(&contr_b.name) {
            return None;
        }
        let hist = self.spread_histories.get(&pair)?;
        if hist.len() < self.params.lookback_zscore {
            return None;
        }
        let z = self.calc_z(hist)?;

        if self.params.debug && z.abs() > self.params.entry_z * 0.5 {
            eprintln!(
                "Candidate {:?} z={:.3} (threshold {})",
                pair, z, self.params.entry_z
            );
        }
        if z.abs() <= self.params.entry_z {
            return None;
        }
        let kind = if z > 0.0 {
            PositionKind::Short
        } else {
            PositionKind::Long
        };

        let status = broker.get_status();
        if status.cash <= 0.0 {
            return None;
        }
        const LEVERAGE: f32 = 3.0;
        let safe_exposure = LEVERAGE * status.equity - status.gross_exposure;
        let leg_prices = contr_a.price.abs() + contr_b.price.abs();
        let mut size: u32 = (safe_exposure.min(status.cash) / leg_prices).floor() as u32;

        let vol_cap = contr_a.volume.min(contr_b.volume) as f32 * 0.01; // 1% of lesser volume
        let vol_cap_u = vol_cap.floor() as u32;
        size = size.min(vol_cap_u);

        if size <= 0 {
            return None;
        }
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
                kind,
                entry_bar: self.bar_count,
                entry_spread: self.cur_price(&pair)?,
                entry_z: z,
                gross_notional: size as f32 * leg_prices,
                size: size,
            },
        );
        Some(())
    }

    fn try_enter_positions(
        &mut self,
        mut a: ContractsToday,
        mut b: ContractsToday,
        broker: &mut dyn Broker,
    ) -> Option<()> {
        const MAX_CONTRACTS: usize = 5;
        a.sort_by_key(|c| std::cmp::Reverse(c.volume));
        b.sort_by_key(|c| std::cmp::Reverse(c.volume));
        a.truncate(a.len().min(MAX_CONTRACTS));
        b.truncate(b.len().min(MAX_CONTRACTS));

        for contr_a in a.iter() {
            for contr_b in b.iter() {
                self.try_enter(contr_a, contr_b, broker);
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
            self.close(pair, pos, "expiry", broker)?;
        }
        Some(())
    }

    fn stop_loss(&mut self, broker: &mut dyn Broker) -> Option<()> {
        // Risk cap: e.g. 5% of capital allocated to the trade (position-specific)
        const LOSS_RATIO_THRESHOLD: f32 = -0.05;
        let mut to_close: Vec<(String, String)> = Vec::new();
        for (pair, pos) in self.active_positions.iter() {
            let cur_price = self.cur_price(pair)?;
            let trade_ret = match pos.kind {
                PositionKind::Long => cur_price - pos.entry_spread,
                PositionKind::Short => pos.entry_spread - cur_price,
            };
            let pnl = trade_ret * pos.size as f32;

            if pnl < LOSS_RATIO_THRESHOLD * pos.gross_notional {
                to_close.push(pair.clone());
            }
        }
        for pair in to_close {
            let pos = self.active_positions.remove(&pair)?;
            self.close(pair, pos, "stop loss", broker)?;
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use polars::prelude::*;
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::PathBuf;

    use crate::engine::{AccountStatus, ContractData};

    use super::*;

    // Minimal mock broker implementing the Broker trait used by PairStrategy.
    // It keeps static cash/equity so sizing logic is deterministic and does not
    // attempt to model PnL (not needed for unit tests of decision logic).
    struct TestBroker {
        cash: f32,
        equity: f32,
        // Record symbols we traded for simple sanity checks if needed.
        traded: HashSet<String>,
    }

    impl TestBroker {
        fn new() -> Self {
            Self {
                cash: 100_000.0,
                equity: 100_000.0,
                traded: HashSet::new(),
            }
        }
    }

    impl Broker for TestBroker {
        fn buy(&mut self, symbol: &str, _qty: u32) -> Option<i32> {
            self.traded.insert(format!("BUY:{symbol}"));
            Some(0)
        }
        fn sell(&mut self, symbol: &str, _qty: u32) -> Option<i32> {
            self.traded.insert(format!("SELL:{symbol}"));
            Some(0)
        }
        fn get_status(&'_ self) -> AccountStatus {
            AccountStatus {
                cash: self.cash,
                equity: self.equity,
                gross_exposure: self.equity,
            }
        }
    }

    fn base_params() -> Params {
        Params {
            lookback_zscore: 5,
            entry_z: 1.5,
            exit_z: 0.5,
            expiry_close_days: 3,
            debug: false,
            commodity_a_prefix: "A".into(),
            commodity_b_prefix: "B".into(),
            transaction_cost_pct: 0.0,
        }
    }

    #[test]
    fn load_history_same_prefix_intra_pairs() {
        // Both prefixes identical -> intra commodity pairing
        let mut params = base_params();
        params.commodity_a_prefix = "ag".into();
        params.commodity_b_prefix = "ag".into();
        // Build simple 2 days, 3 contracts per day -> expect 3 unique pair spreads per day (C(3,2)=3)
        let df = df!(
            "Date" => &["2025-08-06", "2025-08-06", "2025-08-06", "2025-08-07", "2025-08-07", "2025-08-07"],
            "Contract" => &["ag2510", "ag2511", "ag2512", "ag2510", "ag2511", "ag2512"],
            "Price" => &[9182.0_f64, 9191.0, 9205.0, 9185.0, 9190.0, 9210.0]
        ).unwrap();
        let strat_params = params.clone();
        let mut strategy = PairStrategy::new(strat_params, HashMap::new());
        strategy.load_spread_history(df.lazy()).expect("load history");
        // Expect 3 pairs: ag2510/ag2511, ag2510/ag2512, ag2511/ag2512
        assert_eq!(
            strategy.spread_histories.len(),
            3,
            "should have 3 intra pairs"
        );
        for hist in strategy.spread_histories.values() {
            assert_eq!(hist.len(), 2, "two days of spreads");
        }
    }

    #[test]
    fn load_history_cross_prefix_pairs() {
        let mut params = base_params();
        params.commodity_a_prefix = "ag".into();
        params.commodity_b_prefix = "cu".into();
        // 2 days, 2 AG contracts, 2 CU contracts each day -> 4 spreads per day (2x2)
        let df = df!(
            "Date" => &["2025-08-06","2025-08-06","2025-08-06","2025-08-06","2025-08-07","2025-08-07","2025-08-07","2025-08-07"],
            "Contract" => &["ag2510","ag2511","cu2510","cu2511","ag2510","ag2511","cu2510","cu2511"],
            "Price" => &[10.0_f64,11.0,20.0,21.0,11.0,12.0,19.0,22.0]
        ).unwrap();
        let mut strat = PairStrategy::new(params.clone(), HashMap::new());
        strat.load_spread_history(df.lazy()).expect("load history");
        // Expect 4 unique AGxCU pairs
        assert_eq!(strat.spread_histories.len(), 4, "should have 4 cross pairs");
        for hist in strat.spread_histories.values() {
            assert_eq!(hist.len(), 2, "two days of spreads");
        }
    }

    fn mk_contract(name: &str, price: f32, volume: u32) -> ContractData {
        ContractData {
            name: name.to_string(),
            price,
            volume,
        }
    }

    // Helper to run one bar through strategy for single pair (a,b)
    fn run_bar(
        strategy: &mut PairStrategy,
        broker: &mut dyn Broker,
        bar: u32,
        price_a: f32,
        price_b: f32,
        name_a: &str,
        name_b: &str,
        vol: u32,
    ) {
        let a_vec = vec![mk_contract(name_a, price_a, vol)];
        let b_vec = vec![mk_contract(name_b, price_b, vol)];
        strategy.trade(bar, a_vec, b_vec, broker).expect("trade ok");
    }

    #[test]
    fn enters_long_position_on_negative_extreme_z() {
        let params = base_params();
        let mut expiry: HashMap<String, u32> = HashMap::new();
        expiry.insert("A1".into(), 50);
        expiry.insert("B1".into(), 50);
        let mut strat = PairStrategy::new(params, expiry);
        let mut broker = TestBroker::new();

        // Build 5-bar history where final spread is an outlier negative enough to cross entry_z.
        // Spread = price_a - price_b.
        let spreads = [-1.0, -1.1, -0.9, -1.0, -3.0];
        for (i, s) in spreads.iter().enumerate() {
            // Choose price_a and price_b such that difference equals s. Use price_b = 10.0.
            run_bar(
                &mut strat,
                &mut broker,
                (i + 1) as u32,
                10.0 + s,
                10.0,
                "A1",
                "B1",
                10_000,
            );
        }

        // After last bar a position should be opened.
        assert_eq!(
            strat.active_positions.len(),
            1,
            "Expected one active position"
        );
        let pos = strat.active_positions.values().next().unwrap();
        assert!(
            matches!(pos.kind, PositionKind::Long),
            "Expected Long position due to negative z"
        );
        // Volume cap: 1% of 10_000 = 100.
        assert_eq!(pos.size, 100, "Size should respect 1% volume cap");
    }

    #[test]
    fn reversion_closes_position() {
        let params = base_params();
        let mut expiry: HashMap<String, u32> = HashMap::new();
        expiry.insert("A1".into(), 50);
        expiry.insert("B1".into(), 50);
        let mut strat = PairStrategy::new(params, expiry);
        let mut broker = TestBroker::new();

        // Enter (same sequence as previous test)
        let spreads = [-1.0, -1.1, -0.9, -1.0, -3.0];
        for (i, s) in spreads.iter().enumerate() {
            run_bar(
                &mut strat,
                &mut broker,
                (i + 1) as u32,
                10.0 + s,
                10.0,
                "A1",
                "B1",
                10_000,
            );
        }
        assert_eq!(strat.active_positions.len(), 1, "Entry failed");

        // Provide new bars moving spread back toward mean (e.g., -1.0 repeatedly) to trigger reversion exit (z magnitude < exit_z)
        for add in 0..3 {
            // a few bars to dilute outlier
            let bar = (spreads.len() + 1 + add) as u32;
            run_bar(&mut strat, &mut broker, bar, 9.0, 10.0, "A1", "B1", 10_000);
            // spread = -1.0
        }

        assert_eq!(
            strat.active_positions.len(),
            0,
            "Position should have been closed on reversion"
        );
        assert_eq!(strat.trade_log.len(), 1, "One trade should be logged");
        assert_eq!(strat.trade_log[0].reason, "reversion");
    }

    #[test]
    fn stop_loss_triggers_for_short_position() {
        let mut params = base_params();
        params.entry_z = 1.5; // ensure entry on positive outlier
        let mut expiry: HashMap<String, u32> = HashMap::new();
        expiry.insert("A1".into(), 50);
        expiry.insert("B1".into(), 50);
        let mut strat = PairStrategy::new(params, expiry);
        let mut broker = TestBroker::new();

        // Build spreads with last large positive outlier -> Short entry
        let spreads = [1.0, 1.1, 0.9, 1.0, 3.0];
        for (i, s) in spreads.iter().enumerate() {
            run_bar(
                &mut strat,
                &mut broker,
                (i + 1) as u32,
                10.0 + s,
                10.0,
                "A1",
                "B1",
                10_000,
            );
        }
        assert_eq!(strat.active_positions.len(), 1, "Short entry expected");
        let entry_spread = strat.active_positions.values().next().unwrap().entry_spread;
        assert!(entry_spread > 0.0);

        // Adverse move: increase spread further so unrealized loss breaches 5% stop threshold
        // Provide one more bar with even larger spread (e.g. 5.0) so PnL = -2.0 * size < -0.05 * gross_notional
        run_bar(&mut strat, &mut broker, 6, 15.0, 10.0, "A1", "B1", 10_000); // spread 5.0

        assert_eq!(
            strat.active_positions.len(),
            0,
            "Stop loss should close position"
        );
        assert_eq!(strat.trade_log.len(), 1);
        assert_eq!(
            strat.trade_log[0].reason, "stop loss",
            "Expected stop loss reason, got {}",
            strat.trade_log[0].reason
        );
    }

    #[test]
    fn expiry_forces_close() {
        let params = base_params();
        // Expiry on day 10, close window 3 -> any bar with cur_day >= 7 triggers force close.
        let mut expiry: HashMap<String, u32> = HashMap::new();
        expiry.insert("A1".into(), 10);
        expiry.insert("B1".into(), 10);
        let mut strat = PairStrategy::new(params, expiry);
        let mut broker = TestBroker::new();

        // Enter a position early (same negative outlier pattern)
        let spreads = [-1.0, -1.1, -0.9, -1.0, -3.0];
        for (i, s) in spreads.iter().enumerate() {
            run_bar(
                &mut strat,
                &mut broker,
                (i + 1) as u32,
                10.0 + s,
                10.0,
                "A1",
                "B1",
                10_000,
            );
        }
        assert_eq!(
            strat.active_positions.len(),
            0,
            "shouldn't enter when expiry is near"
        );
    }

    #[test]
    fn close_applies_transaction_costs() {
        // Params with non-zero transaction cost
        let mut params = base_params();
        params.transaction_cost_pct = 0.001; // 10 bps per leg
                                             // Ensure exit_z low enough to force exit after outlier reverts
        params.exit_z = 0.5;
        let mut expiry: HashMap<String, u32> = HashMap::new();
        expiry.insert("A1".into(), 50);
        expiry.insert("B1".into(), 50);
        let mut strat = PairStrategy::new(params.clone(), expiry);
        let mut broker = TestBroker::new();

        // Build history to trigger long entry on negative outlier spread.
        // We'll control prices so spread sequence ends with a large negative to enter,
        // then we add one reverting bar to trigger close via reversion logic.
        let spreads = [-1.0, -1.1, -0.9, -1.0, -3.0];
        for (i, s) in spreads.iter().enumerate() {
            run_bar(
                &mut strat,
                &mut broker,
                (i + 1) as u32,
                10.0 + s, // price_a
                10.0,     // price_b (fixed)
                "A1",
                "B1",
                20_000, // higher volume so size cap = 1% = 200
            );
        }
        // Position should be open
        assert_eq!(strat.active_positions.len(), 1, "Expected open position");
        let pos_snapshot = {
            let (_k, v) = strat.active_positions.iter().next().unwrap();
            v.clone()
        };
        assert!(matches!(pos_snapshot.kind, PositionKind::Long));

        // Record entry data for later expected PnL calculation
        let entry_spread = pos_snapshot.entry_spread; // should be last spread (-3.0)
        let size = pos_snapshot.size as f32; // expected 200 (1% of 20_000)
        let gross_notional = pos_snapshot.gross_notional; // size * (price_a + price_b) at entry

        // Add a reverting bar: spread moves back near -1.0 so z will shrink below exit_z
        run_bar(
            &mut strat,
            &mut broker,
            (spreads.len() + 1) as u32,
            9.0,  // price_a so spread = -1.0
            10.0, // price_b
            "A1",
            "B1",
            20_000,
        );

        // Position should now be closed and exactly one trade logged.
        assert_eq!(strat.active_positions.len(), 0, "Position not closed");
        assert_eq!(strat.trade_log.len(), 1, "Expected one trade log entry");
        let trade = &strat.trade_log[0];
        assert_eq!(trade.reason, "reversion");

        // Expected gross PnL: (exit_spread - entry_spread) * size  (Long position)
        // entry_spread ≈ -3.0, exit_spread ≈ -1.0 => diff = 2.0
        let expected_gross = (trade.exit_spread - entry_spread) * size; // 2.0 * size
                                                                        // Costs: 2 * transaction_cost_pct * gross_notional
        let expected_costs = 2.0 * params.transaction_cost_pct * gross_notional;
        let expected_net = expected_gross - expected_costs;

        // Allow tiny float tolerance
        let tol = 1e-4;
        assert!(
            (trade.ret - expected_net).abs() < tol,
            "ret mismatch: got {:.6}, expected {:.6} (gross {:.6} costs {:.6})",
            trade.ret,
            expected_net,
            expected_gross,
            expected_costs
        );

        // ret_pct should be net / gross_notional
        let expected_ret_pct = expected_net / gross_notional;
        assert!(
            (trade.ret_pct - expected_ret_pct).abs() < tol,
            "ret_pct mismatch: got {:.6}, expected {:.6}",
            trade.ret_pct,
            expected_ret_pct
        );

        // Basic sanity: gross should be positive, costs positive, net slightly less than gross
        assert!(expected_gross > 0.0 && expected_costs > 0.0 && expected_net < expected_gross);
    }
}
