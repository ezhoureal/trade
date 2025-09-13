#[derive(Clone, Debug)]
pub struct Params {
    pub lookback_zscore: usize,
    pub lookback_performance: usize,
    pub entry_z: f64,
    pub exit_z: f64,
    pub expiry_close_days: usize, // days before contract's last appearance to force close
    pub debug: bool,
    // New: configurable commodity prefixes (default values supplied by CLI layer)
    pub commodity_a_prefix: String,
    pub commodity_b_prefix: String,
}
