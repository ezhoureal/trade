use super::PairStrategy;
use crate::params::Params;
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use polars::prelude::*;
use std::collections::HashMap;
use std::fs::File;

// Helper type to persist active_positions where the key is a tuple (String, String).
// JSON object keys must be strings, so we store as a Vec of records instead.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct SerializablePosition {
    a: String,
    b: String,
    pos: super::PairPosition,
}

#[cfg(feature = "live")]
impl PairStrategy {
    pub fn new_live(params: Params, df: LazyFrame) -> Self {
        use std::collections::HashMap;
        let single_commodity = params.commodity_a_prefix == params.commodity_b_prefix;
        let mut strategy = PairStrategy {
            params,
            spread_histories: HashMap::new(),
            active_positions: HashMap::new(),
            contract_expiry: HashMap::new(),
            bar_count: 0,
            trade_log: Vec::new(),
            single_commodity,
        };
        strategy
            .load_spread_history(df)
            .expect("Failed to load spread history");
        let _ = strategy.load_positions();
        strategy
    }

    pub fn set_expiry_dates(
        &mut self,
        current_date: &NaiveDate,
        instrument_expiry: &HashMap<String, NaiveDate>,
    ) {
        // Helper: count trading days (Mon-Fri) between start (exclusive) and end (inclusive of end-1).
        fn trading_days_between(start: NaiveDate, end: NaiveDate) -> u32 {
            if end <= start {
                return 0;
            }
            let mut d = start;
            let mut count: u32 = 0;
            while d < end {
                match d.weekday() {
                    Weekday::Sat | Weekday::Sun => {}
                    _ => count += 1,
                }
                d = d + Duration::days(1);
            }
            count
        }

        let mut local: HashMap<String, u32> = HashMap::new();
        for (inst, exp_date) in instrument_expiry.iter() {
            let days = trading_days_between(*current_date, *exp_date);
            local.insert(inst.clone(), days);
        }
        self.contract_expiry = local;
    }

    #[cfg(feature = "live")]
    pub fn pop_spread(&mut self) {
        for (_, history) in self.spread_histories.iter_mut() {
            history.pop_back();
        }
    }

    fn load_positions(&mut self) -> std::io::Result<()> {
        let mut file = File::open("positions.json")?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        if let Ok(list) = serde_json::from_str::<Vec<SerializablePosition>>(&contents) {
            self.active_positions.clear();
            for item in list {
                self.active_positions.insert((item.a, item.b), item.pos);
            }
        }
        println!("Loaded positions: {:?}", self.active_positions);
        Ok(())
    }

    pub fn get_positions(&self) -> &HashMap<(String, String), super::PairPosition> {
        &self.active_positions
    }

    pub fn save_positions(&self) {
        // Save current positions to a file as a Vec of records to avoid non-string map keys.
        use std::fs::File;
        use std::io::Write;

        let list: Vec<SerializablePosition> = self
            .active_positions
            .iter()
            .map(|((a, b), pos)| SerializablePosition {
                a: a.clone(),
                b: b.clone(),
                pos: pos.clone(),
            })
            .collect();

        let positions_json =
            serde_json::to_string_pretty(&list).expect("Failed to serialize positions");
        let mut file = File::create("positions.json").expect("Failed to create positions file");
        file.write_all(positions_json.as_bytes())
            .expect("Failed to write positions to file");
    }

    fn load_spread_history(&mut self, df: LazyFrame) -> PolarsResult<()> {
        use polars::prelude::SortMultipleOptions;

        let sort_opts = SortMultipleOptions {
            descending: vec![false],
            maintain_order: false,
            nulls_last: vec![false],
            multithreaded: true,
            limit: None,
        };
        let sorted = df.sort_by_exprs(vec![col("Date")], sort_opts).collect()?;
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
                    self.flush_day(&a_contracts, &b_contracts);
                    a_contracts.clear();
                    b_contracts.clear();
                }
            }
            last_date = Some(cur_date);
            if let Some(contract) = contracts_s.get(row) {
                if let Some(price_v) = prices_f.get(row) {
                    let price = price_v as f32;
                    let c_l = contract.to_lowercase();
                    if c_l.starts_with(&self.params.commodity_a_prefix) {
                        a_contracts.push((contract.to_string(), price));
                    }
                    if c_l.starts_with(&self.params.commodity_b_prefix) {
                        b_contracts.push((contract.to_string(), price));
                    }
                }
            }
        }
        self.flush_day(&a_contracts, &b_contracts);
        Ok(())
    }

    fn flush_day(&mut self, a_contracts: &Vec<(String, f32)>, b_contracts: &Vec<(String, f32)>) {
        for (name_a, price_a) in a_contracts.iter() {
            for (name_b, price_b) in b_contracts.iter() {
                if name_a == name_b {
                    continue; // skip same contract pairs
                }
                let spread = *price_a - *price_b;
                self.push_spread((name_a.clone(), name_b.clone()), spread);
            }
        }
    }
}

#[cfg(all(test, feature = "live"))]
mod test {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn load_history_cross_prefix_pairs() {
        let mut params = Params::default();
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

    #[test]
    fn load_history_same_prefix_intra_pairs() {
        // Both prefixes identical -> intra commodity pairing
        let mut params = Params::default();
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
        strategy
            .load_spread_history(df.lazy())
            .expect("load history");
        assert_eq!(
            strategy.spread_histories.len(),
            6,
            "should have 6 intra pairs"
        );
        for hist in strategy.spread_histories.values() {
            assert_eq!(hist.len(), 2, "two days of spreads");
        }
    }

    #[test]
    fn set_expiry_dates_counts_weekdays() {
        use chrono::NaiveDate;
        // current date is a Friday
        let current = NaiveDate::from_ymd_opt(2025, 9, 19).unwrap(); // 2025-09-19 (Fri)

        let mut params = Params::default();
        let mut strat = PairStrategy::new(params.clone(), HashMap::new());

        let mut inst_expiry: HashMap<String, NaiveDate> = HashMap::new();
        // Same-day expiry -> 0
        inst_expiry.insert(
            "ag2510".into(),
            NaiveDate::from_ymd_opt(2025, 9, 19).unwrap(),
        );
        // Following Monday -> counts Friday only -> 1
        inst_expiry.insert(
            "ag2511".into(),
            NaiveDate::from_ymd_opt(2025, 9, 22).unwrap(),
        );
        // Next Friday -> Fri (19), Mon-Thu (22-25) => 5 trading days
        inst_expiry.insert(
            "cu2510".into(),
            NaiveDate::from_ymd_opt(2025, 9, 26).unwrap(),
        );
        // Past expiry -> 0
        inst_expiry.insert(
            "cu2509".into(),
            NaiveDate::from_ymd_opt(2025, 9, 18).unwrap(),
        );

        strat.set_expiry_dates(&current, &inst_expiry);

        assert_eq!(strat.contract_expiry.get("ag2510").copied(), Some(0));
        assert_eq!(strat.contract_expiry.get("ag2511").copied(), Some(1));
        assert_eq!(strat.contract_expiry.get("cu2510").copied(), Some(5));
        assert_eq!(strat.contract_expiry.get("cu2509").copied(), Some(0));
    }
}
