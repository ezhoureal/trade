#[derive(Clone, Debug)]
pub struct Params {
    pub lookback_zscore: usize,
    pub lookback_performance: usize,
    pub entry_z: f64,
    pub exit_z: f64,
    pub pair_evaluation_freq: usize,
    pub max_active_pairs: usize,
    pub min_volume_threshold: usize,
    pub exploration_rate: f64,
    pub debug: bool,
}
