use china_stock::{RealTimeDataFeed, DataSource, SimulatedExecution, real_time_strategy::{OrderRequest, MarketData}};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    
    println!("Chinese Stock Market Real-Time Strategy Testing");
    println!("==============================================");
    
    // Set up real-time data feed
    let data_feed = RealTimeDataFeed::new(DataSource::Tushare, "your_api_key_here".to_string());
    
    // Set up simulated execution engine with initial capital
    let mut execution_engine = SimulatedExecution::new(1_000_000.0); // 1 million RMB
    
    // Define symbols to watch
    let symbols = vec!["000001.SZ".to_string(), "600519.SH".to_string(), "002594.SZ".to_string()];
    
    println!("Initial capital: {:.2} RMB", execution_engine.initial_capital);
    println!("Watching symbols: {:?}", symbols);
    
    // Fetch real-time market data
    match data_feed.get_real_time_data(&symbols).await {
        Ok(market_data_list) => {
            println!("Retrieved real-time market data:");
            for data in &market_data_list {
                println!("  {}: {:.2} ({}%) Bid:{:.2} Ask:{:.2}", 
                    data.name, 
                    data.price, 
                    data.涨跌幅.unwrap_or(0.0),
                    data.bid_price,
                    data.ask_price
                );
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
                
                println!("\nExecuting strategy: BUY signal for {}", market_data.symbol);
                match execution_engine.execute_order(&buy_order, market_data) {
                    Ok(order_response) => {
                        println!("✓ Simulated order executed: {:?}", order_response);
                        
                        // Print current performance
                        let metrics = execution_engine.get_performance_metrics();
                        println!("\nPortfolio Metrics:");
                        println!("  Total P&L: {:.2} RMB ({:.2}%)", metrics.total_pnl, metrics.total_return);
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
                
                println!("\nExecuting strategy: SELL signal for {}", market_data.symbol);
                match execution_engine.execute_order(&sell_order, market_data) {
                    Ok(order_response) => {
                        println!("✓ Simulated sell order executed: {:?}", order_response);
                        
                        // Print updated performance
                        let metrics = execution_engine.get_performance_metrics();
                        println!("\nUpdated Portfolio Metrics:");
                        println!("  Total P&L: {:.2} RMB ({:.2}%)", metrics.total_pnl, metrics.total_return);
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
    
    println!("\nReal-time strategy testing completed.");
    
    Ok(())
}