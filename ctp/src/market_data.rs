use backtest::engine::ContractData;
use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

#[derive(Clone, Copy, Debug)]
pub struct InstrumentQuote {
    pub last_price: f32,
    pub volume: u32,
}

// Keys stored in lowercase for consistency with user preference
pub static MARKET_DATA: LazyLock<RwLock<HashMap<String, InstrumentQuote>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn update_quote(instrument: &str, last_price: f32, volume: u32) {
    let key = instrument.to_ascii_lowercase();
    if let Ok(mut map) = MARKET_DATA.write() {
        map.insert(key, InstrumentQuote { last_price, volume });
    }
}

pub fn get_price(instrument: &str) -> Option<f32> {
    let key = instrument.to_ascii_lowercase();
    MARKET_DATA
        .read()
        .ok()
        .and_then(|m| m.get(&key).map(|q| q.last_price))
}

pub fn snapshot_contracts() -> Vec<ContractData> {
    if let Ok(map) = MARKET_DATA.read() {
        map.iter()
            .map(|(name, q)| ContractData {
                name: name.clone(),
                price: q.last_price,
                volume: q.volume,
            })
            .collect()
    } else {
        Vec::new()
    }
}
