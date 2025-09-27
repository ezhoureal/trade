use std::default;

#[derive(Clone, Debug)]
pub struct Commodity {
    pub name: String,
    pub multiplier: f32,
    pub transaction_cost: f32,
    pub margin_ratio: f32,
}

impl default::Default for Commodity {
    fn default() -> Self {
        Commodity {
            name: "ag".into(),
            multiplier: 15.0,
            transaction_cost: 0.00005,
            margin_ratio: 0.14,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Params {
    pub lookback_zscore: usize,
    pub entry_z: f32,
    pub exit_z: f32,
    pub expiry_close_days: u32, // days before contract's last appearance to force close
    pub debug: bool,
    pub a: Commodity,
    pub b: Commodity,
    pub hedge_ratio: f32, // ratio of commodity B to commodity A in the pair
}

impl default::Default for Params {
    fn default() -> Self {
        Params {
            lookback_zscore: 5,
            entry_z: 1.5,
            exit_z: 0.5,
            expiry_close_days: 3,
            debug: false,
            a: Commodity::default(),
            b: Commodity::default(),
            hedge_ratio: 1.0,
        }
    }
}
