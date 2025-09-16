#[derive(Clone, Debug)]
pub struct Params {
    pub lookback_zscore: usize,
    pub entry_z: f32,
    pub exit_z: f32,
    pub expiry_close_days: u32, // days before contract's last appearance to force close
    pub debug: bool,
    pub commodity_a_prefix: String,
    pub commodity_b_prefix: String,
}
