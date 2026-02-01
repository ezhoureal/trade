# Chinese Stock Market Module

This module provides a simulation interface for the Chinese stock market, allowing you to develop and test trading strategies against real-time market data.

## Features

- **Real-time Data Integration**: Connect to real Chinese stock market data sources (Tushare, AkShare, Baostock)
- **Simulated Execution Engine**: Test trading strategies without real money
- **Accurate Market Simulation**: Includes slippage, transaction fees, bid/ask spreads
- **Performance Tracking**: Monitor strategy performance with detailed metrics
- **Risk Management**: Built-in validation for funds and positions

## Architecture

The module consists of:

1. **Real-time Data Feed** (`RealTimeDataFeed`): Connects to external data sources
2. **Simulated Execution Engine** (`SimulatedExecution`): Executes trades with realistic market conditions
3. **Market Data Structures** (`MarketData`): Comprehensive stock information
4. **Order Management**: Supports LIMIT, MARKET, and other order types

## Data Sources Supported

- **Tushare**: Professional financial data provider
- **AkShare**: Free, open-source financial data toolkit
- **Baostock**: Free stock data source
- **Custom**: Connect to your own data provider

## Usage

### Rust Binary
```bash
cargo run -p china_stock -- --api-key YOUR_API_KEY --secret-key YOUR_SECRET_KEY --environment sim
```

### Real-time Strategy Testing
The module includes a Python script for real-time strategy testing:
```bash
python test_real_time_strategy.py
```

## Real-time Strategy Testing

The `test_real_time_strategy.py` script demonstrates how to:

1. **Connect to Real Data**: Fetch live market data from Chinese exchanges
2. **Apply Trading Strategies**: Implement algorithmic trading logic
3. **Execute Simulated Trades**: Test strategies with realistic market conditions
4. **Monitor Performance**: Track P&L, returns, and other metrics

### Example Strategy Included
- Moving average crossover strategy
- Risk management (position sizing, stop-loss)
- Performance reporting

## Configuration

The module supports different environments:
- `sim`: Simulation environment for testing
- `paper`: Paper trading environment (future implementation)
- `live`: Live trading (not recommended for initial development)

## Data Fields

The module provides comprehensive stock data including:
- Symbol and company name
- Current price with percentage change (涨跌幅)
- Price change amount (涨跌额)
- OHLC (Open, High, Low, Close) prices
- Bid/Ask prices and volumes
- Volume and turnover data
- Timestamp information

## Implementation Details

This is a hybrid implementation that:
- Provides a realistic simulation environment for development
- Connects to real market data sources for accurate testing
- Includes realistic market microstructure (slippage, fees, spreads)
- Maintains the same interface you would find in a production system

## Getting Started

1. Install dependencies:
   ```bash
   pip install akshare  # For Python data integration
   ```

2. Configure your data source API keys (for paid services like Tushare)

3. Run the real-time strategy test:
   ```bash
   python test_real_time_strategy.py
   ```

4. Develop your own strategies by modifying the example strategy function

This approach allows you to test your trading strategies against real market conditions while maintaining a safe simulation environment.