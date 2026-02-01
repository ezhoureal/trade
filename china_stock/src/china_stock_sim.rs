use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio;
use log::{info, error};
use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StockAccountInfo {
    pub account_id: String,
    pub balance: f64,
    pub available: f64,
    pub frozen: f64,
    pub market_value: f64,
    pub total_assets: f64,
    pub currency: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: String, // "BUY" or "SELL"
    pub quantity: u32,
    pub price: f64,
    pub order_type: String, // "LIMIT", "MARKET", "BEST_LIMIT"
    pub entrust_type: String, // "E" for normal order
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OrderResponse {
    pub order_id: String,
    pub status: String,
    pub symbol: String,
    pub filled_quantity: u32,
    pub avg_fill_price: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketData {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
    pub turnover: f64,
    pub timestamp: String,
    pub 涨跌幅: Option<f64>, // Price change percentage
    pub 涨跌额: Option<f64>, // Price change amount
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Position {
    pub symbol: String,
    pub name: String,
    pub quantity: i32,
    pub available: i32,
    pub avg_cost: f64,
    pub market_value: f64,
    pub profit_loss: f64,
}

#[derive(Clone)]
pub struct ChinaStockSimClient {
    pub api_key: String,
    pub secret_key: String,
    pub environment: String,
    // Mock data for demonstration
    pub mock_accounts: HashMap<String, StockAccountInfo>,
    pub mock_positions: HashMap<String, Vec<Position>>,
    pub mock_orders: HashMap<String, OrderResponse>,
}

#[derive(Debug, thiserror::Error)]
pub enum ChinaStockError {
    #[error("API error: {0}")]
    Api(String),
    
    #[error("Authentication error: {0}")]
    Auth(String),
    
    #[error("Validation error: {0}")]
    Validation(String),
}

impl ChinaStockSimClient {
    pub fn new(api_key: String, secret_key: String, environment: String) -> Self {
        let mut mock_accounts = HashMap::new();
        let mock_positions = HashMap::new();
        
        // Initialize mock data
        let account = StockAccountInfo {
            account_id: "SIM_ACC_001".to_string(),
            balance: 1000000.0,
            available: 950000.0,
            frozen: 0.0,
            market_value: 50000.0,
            total_assets: 1000000.0,
            currency: "CNY".to_string(),
        };
        mock_accounts.insert("SIM_ACC_001".to_string(), account);
        
        Self {
            api_key,
            secret_key,
            environment,
            mock_accounts,
            mock_positions,
            mock_orders: HashMap::new(),
        }
    }

    pub async fn connect(&self) -> Result<bool, ChinaStockError> {
        info!("Connecting to Chinese stock market simulator...");
        
        // Simulate connection logic
        if self.api_key.is_empty() {
            return Err(ChinaStockError::Auth("API key is required".to_string()));
        }
        
        info!("Successfully connected to Chinese stock market simulator ({})", self.environment);
        Ok(true)
    }

    pub async fn get_account_info(&mut self) -> Result<StockAccountInfo, ChinaStockError> {
        info!("Fetching account information...");
        
        // Return mock account info
        if let Some(account) = self.mock_accounts.get("SIM_ACC_001") {
            info!("Retrieved account info: {:?}", account);
            Ok(account.clone())
        } else {
            Err(ChinaStockError::Api("Account not found".to_string()))
        }
    }

    pub async fn place_order(&mut self, order: &OrderRequest) -> Result<OrderResponse, ChinaStockError> {
        info!("Placing order: {:?}", order);
        
        // Validate order
        if order.quantity == 0 {
            return Err(ChinaStockError::Validation("Quantity must be greater than 0".to_string()));
        }
        
        if order.price <= 0.0 {
            return Err(ChinaStockError::Validation("Price must be greater than 0".to_string()));
        }
        
        // Generate mock order response
        let order_id = format!("ORDER_{}", rand::random::<u32>());
        let order_response = OrderResponse {
            order_id: order_id.clone(),
            status: "PENDING".to_string(),
            symbol: order.symbol.clone(),
            filled_quantity: 0,
            avg_fill_price: None,
        };
        
        self.mock_orders.insert(order_id, order_response.clone());
        
        info!("Order placed successfully: {:?}", order_response);
        Ok(order_response)
    }

    pub async fn get_market_data(&self, symbols: &[String]) -> Result<Vec<MarketData>, ChinaStockError> {
        info!("Fetching market data for symbols: {:?}", symbols);
        
        let mut market_data = Vec::new();
        
        for symbol in symbols {
            // Generate mock market data based on symbol
            let name = match symbol.as_str() {
                "000001.SZ" => "Ping An Bank".to_string(),
                "600519.SH" => "Kweichow Moutai".to_string(),
                "002594.SZ" => "BYD Co.".to_string(),
                _ => format!("Company for {}", symbol),
            };
            
            // Generate mock prices
            let base_price = match symbol.as_str() {
                "000001.SZ" => 15.50,
                "600519.SH" => 1800.00,
                "002594.SZ" => 250.00,
                _ => 50.0 + (rand::random::<f64>() * 100.0),
            };
            
            let open = base_price * (0.98 + rand::random::<f64>() * 0.04); // Random variation
            let high = base_price.max(open) * (1.0 + rand::random::<f64>() * 0.03);
            let low = base_price.min(open) * (0.97 + rand::random::<f64>() * 0.03);
            let price = low + rand::random::<f64>() * (high - low);
            let close = base_price;
            
            let change_pct = ((price - close) / close) * 100.0;
            let change_amt = price - close;
            
            let data = MarketData {
                symbol: symbol.clone(),
                name,
                price,
                open,
                high,
                low,
                close,
                volume: (rand::random::<u64>() % 10000000) + 1000000,
                turnover: (rand::random::<f64>() * 100000000.0) + 10000000.0,
                timestamp: Utc::now().to_rfc3339(),
                涨跌幅: Some(change_pct),
                涨跌额: Some(change_amt),
            };
            
            market_data.push(data);
        }
        
        info!("Retrieved {} market data items", market_data.len());
        Ok(market_data)
    }

    pub async fn get_positions(&self) -> Result<Vec<Position>, ChinaStockError> {
        info!("Fetching positions...");
        
        // Return mock positions
        let positions = vec![
            Position {
                symbol: "000001.SZ".to_string(),
                name: "Ping An Bank".to_string(),
                quantity: 1000,
                available: 1000,
                avg_cost: 15.20,
                market_value: 15500.0,
                profit_loss: 300.0,
            },
            Position {
                symbol: "600519.SH".to_string(),
                name: "Kweichow Moutai".to_string(),
                quantity: 10,
                available: 10,
                avg_cost: 1750.00,
                market_value: 18000.0,
                profit_loss: 500.0,
            }
        ];
        
        info!("Retrieved {} positions", positions.len());
        Ok(positions)
    }

    pub async fn cancel_order(&mut self, order_id: &str) -> Result<bool, ChinaStockError> {
        info!("Canceling order: {}", order_id);
        
        if self.mock_orders.contains_key(order_id) {
            // In a real implementation, this would change the order status
            info!("Order {} canceled successfully", order_id);
            Ok(true)
        } else {
            error!("Order {} not found", order_id);
            Err(ChinaStockError::Api(format!("Order {} not found", order_id)))
        }
    }
    
    // Method to update account info for demo purposes
    pub fn update_account_balance(&mut self, account_id: &str, new_balance: f64) {
        if let Some(account) = self.mock_accounts.get_mut(account_id) {
            account.balance = new_balance;
            account.available = new_balance - (account.total_assets - account.available); // Simple adjustment
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
pub enum Environment {
    /// Simulation environment
    Sim,
    /// Paper trading environment
    Paper,
    /// Live trading environment (not recommended for initial development)
    Live,
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Select run environment
    #[arg(short, long, value_enum, default_value_t = Environment::Sim)]
    pub environment: Environment,

    /// API Key
    #[arg(short, long)]
    pub api_key: String,

    /// Secret Key
    #[arg(short, long)]
    pub secret_key: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    
    let args = Args::parse();
    
    println!("Starting Chinese Stock Market connector with environment: {:?}", args.environment);
    
    let mut client = ChinaStockSimClient::new(
        args.api_key,
        args.secret_key,
        format!("{:?}", args.environment),
    );
    
    // Connect to the market
    match client.connect().await {
        Ok(success) => {
            if success {
                println!("✓ Successfully connected to Chinese stock market simulator");
                
                // Example operations
                match client.get_account_info().await {
                    Ok(account_info) => {
                        println!("✓ Account Info: {:?}", account_info);
                        
                        // Example: Get market data for some popular stocks
                        let symbols = vec!["000001.SZ".to_string(), "600519.SH".to_string(), "002594.SZ".to_string()]; // Ping An Bank, Kweichow Moutai, BYD
                        match client.get_market_data(&symbols).await {
                            Ok(market_data) => {
                                println!("✓ Market data retrieved:");
                                for data in market_data {
                                    println!("  {}: {:.2} ({}%)", 
                                        data.name, 
                                        data.price, 
                                        data.涨跌幅.unwrap_or(0.0)
                                    );
                                }
                                
                                // Example: Place a test order
                                let test_order = OrderRequest {
                                    symbol: "000001.SZ".to_string(), // Ping An Bank
                                    side: "BUY".to_string(),
                                    quantity: 100,
                                    price: 15.50,
                                    order_type: "LIMIT".to_string(),
                                    entrust_type: "E".to_string(),
                                };
                                
                                match client.place_order(&test_order).await {
                                    Ok(order_resp) => {
                                        println!("✓ Test order placed: {:?}", order_resp);
                                        
                                        // Example: Get positions
                                        match client.get_positions().await {
                                            Ok(positions) => {
                                                println!("✓ Positions retrieved:");
                                                for pos in positions {
                                                    println!("  {}: {} shares, avg cost {:.2}, market value {:.2}", 
                                                        pos.name, 
                                                        pos.quantity, 
                                                        pos.avg_cost, 
                                                        pos.market_value
                                                    );
                                                }
                                            },
                                            Err(e) => {
                                                println!("✗ Failed to get positions: {}", e);
                                            }
                                        }
                                    },
                                    Err(e) => {
                                        println!("✗ Failed to place test order: {}", e);
                                    }
                                }
                            },
                            Err(e) => {
                                println!("✗ Failed to get market data: {}", e);
                            }
                        }
                    },
                    Err(e) => {
                        println!("✗ Failed to get account info: {}", e);
                    }
                }
            } else {
                println!("✗ Failed to connect to Chinese stock market simulator");
            }
        },
        Err(e) => {
            println!("✗ Connection error: {}", e);
        }
    }
    
    Ok(())
}