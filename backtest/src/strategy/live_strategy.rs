use super::PairStrategy;
use crate::params::Params;
use polars::prelude::*;
#[cfg(feature = "live")]
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

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
        let _ = strategy.load_positions();
        strategy
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
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::fs::File;
    use std::io::Read;
    use std::io::Write;

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
}
