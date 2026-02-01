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
    pub bid_price: f64,     // Best bid price
    pub ask_price: f64,     // Best ask price
    pub bid_volume: u64,    // Best bid volume
    pub ask_volume: u64,    // Best ask volume
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TradeRecord {
    pub order_id: String,
    pub symbol: String,
    pub side: String,
    pub quantity: u32,
    pub price: f64,
    pub timestamp: String,
    pub fee: f64,
}

#[derive(Clone)]
pub struct ChinaStockSimClient {
    pub api_key: String,
    pub secret_key: String,
    pub environment: String,
    // Mock data for demonstration
    pub mock_accounts: HashMap<String, StockAccountInfo>,
    pub positions: HashMap<String, Position>,
    pub orders: HashMap<String, OrderResponse>,
    pub trade_records: Vec<TradeRecord>,
    pub current_market_data: HashMap<String, MarketData>,
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

// Real-time data feed - in a real implementation this would connect to external sources
#[derive(Clone)]
pub struct RealTimeDataFeed {
    pub data_source: DataSource,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub enum DataSource {
    Tushare,
    AkShare,
    Baostock,
    Custom { url: String },
}

impl RealTimeDataFeed {
    pub fn new(data_source: DataSource, api_key: String) -> Self {
        Self {
            data_source,
            api_key,
        }
    }

    pub async fn get_real_time_data(&self, symbols: &[String]) -> Result<Vec<MarketData>, ChinaStockError> {
        match self.data_source {
            DataSource::Tushare => self.fetch_from_tushare(symbols).await,
            DataSource::AkShare => self.fetch_from_akshare(symbols).await,
            DataSource::Baostock => self.fetch_from_baostock(symbols).await,
            DataSource::Custom { ref url } => self.fetch_from_custom(url, symbols).await,
        }
    }

    async fn fetch_from_tushare(&self, symbols: &[String]) -> Result<Vec<MarketData>, ChinaStockError> {
        // This would connect to Tushare API in a real implementation
        // For now, we generate mock data based on the symbol
        info!("Fetching mock data for symbols: {:?}", symbols);
        
        let mut market_data = Vec::new();
        for symbol in symbols {
            // Generate mock market data based on symbol
            let name = match symbol.as_str() {
                "000001.SZ" => "Ping An Bank".to_string(),
                "600519.SH" => "Kweichow Moutai".to_string(),
                "002594.SZ" => "BYD Co.".to_string(),
                _ => format!("Company for {}", symbol),
            };

            // Generate mock prices with bid/ask spread
            let base_price = match symbol.as_str() {
                "000001.SZ" => 15.50,
                "600519.SH" => 1800.00,
                "002594.SZ" => 250.00,
                _ => 50.0 + (rand::random::<f64>() * 100.0),
            };
            
            let spread = base_price * 0.001; // 0.1% spread
            let bid_price = base_price - spread/2.0;
            let ask_price = base_price + spread/2.0;
            
            let open = base_price * (0.98 + rand::random::<f64>() * 0.04);
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
                bid_price,
                ask_price,
                bid_volume: (rand::random::<u64>() % 100000) + 10000,
                ask_volume: (rand::random::<u64>() % 100000) + 10000,
            };

            market_data.push(data);
        }

        Ok(market_data)
    }

    async fn fetch_from_akshare(&self, symbols: &[String]) -> Result<Vec<MarketData>, ChinaStockError> {
        // Mock implementation for AkShare
        info!("Fetching mock data from AkShare for symbols: {:?}", symbols);
        self.fetch_from_tushare(symbols).await // Using same mock data for demo
    }

    async fn fetch_from_baostock(&self, symbols: &[String]) -> Result<Vec<MarketData>, ChinaStockError> {
        // Mock implementation for Baostock
        info!("Fetching mock data from Baostock for symbols: {:?}", symbols);
        self.fetch_from_tushare(symbols).await // Using same mock data for demo
    }

    async fn fetch_from_custom(&self, url: &str, symbols: &[String]) -> Result<Vec<MarketData>, ChinaStockError> {
        // Mock implementation for custom data source
        info!("Fetching mock data from custom source {} for symbols: {:?}", url, symbols);
        self.fetch_from_tushare(symbols).await // Using same mock data for demo
    }
}

#[derive(Debug, Clone)]
pub struct SimulatedExecution {
    pub positions: HashMap<String, Position>,
    pub cash_balance: f64,
    pub initial_capital: f64,
    pub historical_trades: Vec<TradeRecord>,
    pub transaction_fee_rate: f64, // e.g., 0.0003 for 0.03%
    pub slippage_rate: f64,        // e.g., 0.0005 for 0.05% slippage
    pub min_transaction_fee: f64,  // minimum fee per transaction
}

impl SimulatedExecution {
    pub fn new(initial_capital: f64) -> Self {
        Self {
            positions: HashMap::new(),
            cash_balance: initial_capital,
            initial_capital,
            historical_trades: Vec::new(),
            transaction_fee_rate: 0.0003, // 0.03% fee
            slippage_rate: 0.0005,        // 0.05% slippage
            min_transaction_fee: 5.0,     // minimum 5 RMB per trade
        }
    }

    pub fn execute_order(
        &mut self,
        order: &OrderRequest,
        market_data: &MarketData
    ) -> Result<OrderResponse, ChinaStockError> {
        info!("Executing order: {:?} at market price: {}", order, market_data.price);

        // Determine execution price based on order type and market conditions
        let execution_price = match order.order_type.as_str() {
            "MARKET" => {
                // For market orders, use ask price for buys, bid price for sells
                if order.side == "BUY" {
                    market_data.ask_price * (1.0 + self.slippage_rate)
                } else {
                    market_data.bid_price * (1.0 - self.slippage_rate)
                }
            },
            "LIMIT" => {
                // For limit orders, check if the limit price is acceptable
                if order.side == "BUY" && order.price >= market_data.ask_price {
                    market_data.ask_price * (1.0 + self.slippage_rate * 0.5) // Less slippage for limit orders
                } else if order.side == "SELL" && order.price <= market_data.bid_price {
                    market_data.bid_price * (1.0 - self.slippage_rate * 0.5)
                } else {
                    // Limit not reached
                    return Err(ChinaStockError::Validation(
                        format!("Limit order not executed: {} vs market", order.price)
                    ));
                }
            },
            _ => market_data.price * if order.side == "BUY" { 1.0 + self.slippage_rate } else { 1.0 - self.slippage_rate },
        };

        // Calculate total cost
        let gross_amount = (execution_price * order.quantity as f64);
        let transaction_fee = (gross_amount * self.transaction_fee_rate).max(self.min_transaction_fee);
        let total_amount = if order.side == "BUY" { 
            gross_amount + transaction_fee 
        } else { 
            gross_amount - transaction_fee 
        };

        // Validate funds for buy orders
        if order.side == "BUY" && total_amount > self.cash_balance {
            return Err(ChinaStockError::Validation(
                format!("Insufficient funds: need {:.2}, have {:.2}", total_amount, self.cash_balance)
            ));
        }

        // Validate position for sell orders
        if order.side == "SELL" {
            if let Some(position) = self.positions.get(&order.symbol) {
                if order.quantity as i32 > position.available {
                    return Err(ChinaStockError::Validation(
                        format!("Insufficient shares to sell: need {}, available {}", 
                               order.quantity, position.available)
                    ));
                }
            } else {
                return Err(ChinaStockError::Validation(
                    format!("No position found for symbol: {}", order.symbol)
                ));
            }
        }

        // Execute the trade
        if order.side == "BUY" {
            self.cash_balance -= total_amount;

            // Update or create position
            let position = self.positions.entry(order.symbol.clone()).or_insert_with(|| {
                Position {
                    symbol: order.symbol.clone(),
                    name: market_data.name.clone(),
                    quantity: 0,
                    available: 0,
                    avg_cost: 0.0,
                    market_value: 0.0,
                    profit_loss: 0.0,
                }
            });

            let old_quantity = position.quantity;
            let old_cost = position.avg_cost * old_quantity as f64;
            let new_quantity = old_quantity + order.quantity as i32;
            let new_cost = old_cost + gross_amount;

            position.quantity = new_quantity;
            position.available = new_quantity; // For simplicity, assume immediate settlement
            position.avg_cost = new_cost / new_quantity as f64;
            position.market_value = execution_price * new_quantity as f64;
        } else if order.side == "SELL" {
            self.cash_balance += total_amount;

            if let Some(position) = self.positions.get_mut(&order.symbol) {
                position.quantity -= order.quantity as i32;
                position.available -= order.quantity as i32;

                // Update market value
                position.market_value = execution_price * position.quantity as f64;
                
                // Update profit/loss calculation
                let sold_cost = position.avg_cost * order.quantity as f64;
                let sold_revenue = execution_price * order.quantity as f64;
                position.profit_loss += sold_revenue - sold_cost;
            }
        }

        // Record the trade
        let trade_record = TradeRecord {
            order_id: format!("TRADE_{}", rand::random::<u32>()),
            symbol: order.symbol.clone(),
            side: order.side.clone(),
            quantity: order.quantity,
            price: execution_price,
            timestamp: Utc::now().to_rfc3339(),
            fee: transaction_fee,
        };

        self.historical_trades.push(trade_record);

        // Create order response
        let order_response = OrderResponse {
            order_id: format!("ORDER_{}", rand::random::<u32>()),
            status: "FILLED".to_string(),
            symbol: order.symbol.clone(),
            filled_quantity: order.quantity,
            avg_fill_price: Some(execution_price),
        };

        info!("Order executed successfully: {:?}", order_response);
        Ok(order_response)
    }

    pub fn get_portfolio_value(&self, market_data: &HashMap<String, MarketData>) -> f64 {
        let mut total_value = self.cash_balance;
        
        for (symbol, position) in &self.positions {
            if let Some(market_price) = market_data.get(symbol) {
                total_value += market_price.price * position.quantity as f64;
            } else {
                // If no market data, use the last known value
                total_value += position.market_value;
            }
        }

        total_value
    }

    pub fn get_performance_metrics(&self) -> PerformanceMetrics {
        let current_value = self.cash_balance + self.positions.values()
            .map(|pos| pos.market_value)
            .sum::<f64>();
        
        let pnl = current_value - self.initial_capital;
        let return_rate = pnl / self.initial_capital * 100.0;

        PerformanceMetrics {
            total_return: return_rate,
            total_pnl: pnl,
            current_portfolio_value: current_value,
            total_trades: self.historical_trades.len(),
            cash_balance: self.cash_balance,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub total_return: f64,           // Total return in percentage
    pub total_pnl: f64,              // Total profit and loss in RMB
    pub current_portfolio_value: f64, // Current total portfolio value
    pub total_trades: usize,         // Total number of trades
    pub cash_balance: f64,           // Current cash balance
}

impl ChinaStockSimClient {
    pub fn new(api_key: String, secret_key: String, environment: String) -> Self {
        let mut mock_accounts = HashMap::new();
        
        // Initialize mock data
        let account = StockAccountInfo {
            account_id: "SIM_ACC_001".to_string(),
            balance: 1000000.0,
            available: 1000000.0,
            frozen: 0.0,
            market_value: 0.0,
            total_assets: 1000000.0,
            currency: "CNY".to_string(),
        };
        mock_accounts.insert("SIM_ACC_001".to_string(), account);

        Self {
            api_key,
            secret_key,
            environment,
            mock_accounts,
            positions: HashMap::new(),
            orders: HashMap::new(),
            trade_records: Vec::new(),
            current_market_data: HashMap::new(),
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

    pub async fn get_market_data(&self, symbols: &[String], data_source: &RealTimeDataFeed) -> Result<Vec<MarketData>, ChinaStockError> {
        data_source.get_real_time_data(symbols).await
    }

    pub fn execute_simulated_order(
        &mut self,
        order: &OrderRequest,
        market_data: &MarketData,
        execution_engine: &mut SimulatedExecution
    ) -> Result<OrderResponse, ChinaStockError> {
        execution_engine.execute_order(order, market_data)
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
    
    println!("Starting Chinese Stock Market Real-time Strategy Testing with environment: {:?}", args.environment);
    
    // Create the main client
    let mut client = ChinaStockSimClient::new(
        args.api_key,
        args.secret_key,
        format!("{:?}", args.environment),
    );
    
    // Set up real-time data feed
    let data_feed = RealTimeDataFeed::new(DataSource::Tushare, client.api_key.clone());
    
    // Set up simulated execution engine with initial capital
    let mut execution_engine = SimulatedExecution::new(1000000.0); // 1 million RMB
    
    // Connect to the market
    match client.connect().await {
        Ok(success) => {
            if success {
                println!("✓ Successfully connected to Chinese stock market simulator");
                
                // Define symbols to watch
                let symbols = vec!["000001.SZ".to_string(), "600519.SH".to_string()];
                
                // Fetch real-time market data
                match data_feed.get_real_time_data(&symbols).await {
                    Ok(market_data_list) => {
                        println!("✓ Retrieved real-time market data:");
                        for data in &market_data_list {
                            println!("  {}: {:.2} ({}%) Bid:{:.2} Ask:{:.2}", 
                                data.name, 
                                data.price, 
                                data.涨跌幅.unwrap_or(0.0),
                                data.bid_price,
                                data.ask_price
                            );
                            
                            // Update client's current market data
                            client.current_market_data.insert(data.symbol.clone(), data.clone());
                        }
                        
                        // Example: Execute a sample buy order based on market data
                        if let Some(market_data) = market_data_list.first() {
                            let buy_order = OrderRequest {
                                symbol: market_data.symbol.clone(),
                                side: "BUY".to_string(),
                                quantity: 1000, // Buy 1000 shares
                                price: market_data.ask_price, // Use ask price for market order simulation
                                order_type: "LIMIT".to_string(),
                                entrust_type: "E".to_string(),
                            };
                            
                            match client.execute_simulated_order(&buy_order, market_data, &mut execution_engine) {
                                Ok(order_response) => {
                                    println!("✓ Simulated order executed: {:?}", order_response);
                                    
                                    // Print current performance
                                    let metrics = execution_engine.get_performance_metrics();
                                    println!("Portfolio Metrics:");
                                    println!("  Total P&L: {:.2} RMB", metrics.total_pnl);
                                    println!("  Return: {:.2}%", metrics.total_return);
                                    println!("  Portfolio Value: {:.2} RMB", metrics.current_portfolio_value);
                                    println!("  Cash Balance: {:.2} RMB", metrics.cash_balance);
                                    println!("  Total Trades: {}", metrics.total_trades);
                                },
                                Err(e) => {
                                    println!("✗ Failed to execute simulated order: {}", e);
                                }
                            }
                        }
                        
                        // Example: Execute a sample sell order
                        if let Some(market_data) = market_data_list.get(1) {
                            let sell_order = OrderRequest {
                                symbol: market_data.symbol.clone(),
                                side: "SELL".to_string(),
                                quantity: 100, // Sell 100 shares
                                price: market_data.bid_price, // Use bid price for market order simulation
                                order_type: "LIMIT".to_string(),
                                entrust_type: "E".to_string(),
                            };
                            
                            match client.execute_simulated_order(&sell_order, market_data, &mut execution_engine) {
                                Ok(order_response) => {
                                    println!("✓ Simulated sell order executed: {:?}", order_response);
                                    
                                    // Print updated performance
                                    let metrics = execution_engine.get_performance_metrics();
                                    println!("Updated Portfolio Metrics:");
                                    println!("  Total P&L: {:.2} RMB", metrics.total_pnl);
                                    println!("  Return: {:.2}%", metrics.total_return);
                                    println!("  Portfolio Value: {:.2} RMB", metrics.current_portfolio_value);
                                    println!("  Cash Balance: {:.2} RMB", metrics.cash_balance);
                                },
                                Err(e) => {
                                    println!("✗ Failed to execute simulated sell order: {}", e);
                                }
                            }
                        }
                    },
                    Err(e) => {
                        println!("✗ Failed to get real-time market data: {}", e);
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