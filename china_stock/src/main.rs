use china_stock::{ChinaStockSimClient, Args, OrderRequest};
use clap::Parser;

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
                            },
                            Err(e) => {
                                println!("✗ Failed to place test order: {}", e);
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