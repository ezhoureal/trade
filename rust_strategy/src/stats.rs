use std::collections::VecDeque;
use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct PairStats {
    pub returns: VecDeque<f64>,
    pub trades: usize,
    pub wins: usize,
    pub total_return: f64,
    pub sharpe: f64,
    pub last_updated: usize,
    pub avg_return_per_trade: f64,
    pub success_score: f64,
    pub lookback: usize,
}

impl PairStats {
    pub fn new(lookback: usize) -> Self {
        Self { returns: VecDeque::with_capacity(lookback), trades:0, wins:0, total_return:0.0, sharpe:0.0, last_updated:0, avg_return_per_trade:0.0, success_score:0.0, lookback }
    }
    pub fn record(&mut self, trade_return: f64, bar_count: usize) {
        if self.returns.len() == self.lookback { self.returns.pop_front(); }
        self.returns.push_back(trade_return);
        self.trades += 1;
        if trade_return > 0.0 { self.wins += 1; }
        self.total_return += trade_return;
        self.last_updated = bar_count;
        if self.trades > 0 { self.avg_return_per_trade = self.total_return / self.trades as f64; }
        if self.returns.len() >= 5 { // compute sharpe
            let mean = self.returns.iter().copied().sum::<f64>() / self.returns.len() as f64;
            let var = self.returns.iter().map(|r| { let d = r - mean; d*d }).sum::<f64>() / self.returns.len() as f64;
            let std = var.sqrt();
            self.sharpe = if std > 0.0 { mean / std } else { 0.0 };
        }
        let win_rate = if self.trades>0 { self.wins as f64 / self.trades as f64 } else { 0.0 };
        self.success_score = self.avg_return_per_trade * 0.4 + win_rate * 0.3 + self.sharpe * 0.3;
    }
}
