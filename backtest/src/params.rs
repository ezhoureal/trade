use std::default;

#[derive(Clone, Debug)]
pub struct Params {
    pub lookback_zscore: usize,
    pub entry_z: f32,
    pub exit_z: f32,
    pub expiry_close_days: u32, // days before contract's last appearance to force close
    pub debug: bool,
    pub commodity_a_prefix: String,
    pub commodity_b_prefix: String,
    pub transaction_cost_pct: f32,
}

impl default::Default for Params {
    fn default() -> Self {
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
}
