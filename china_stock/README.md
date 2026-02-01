# Chinese Stock Market Module

This module provides a simulation interface for the Chinese stock market, allowing you to develop and test trading strategies against a mock version of Chinese exchanges like SSE (Shanghai) and SZSE (Shenzhen).

## Features

- **Mock Trading Interface**: Simulates the functionality of Chinese stock market APIs
- **Account Management**: Track account balances, positions, and orders
- **Market Data**: Access mock market data for Chinese stocks (SH/SZ codes)
- **Order Placement**: Place buy/sell orders with various order types
- **Position Tracking**: Monitor your portfolio holdings

## Supported Stocks

The simulation includes mock data for major Chinese stocks:
- SSE: Shanghai Stock Exchange stocks (e.g., 600519.SH for Kweichow Moutai)
- SZSE: Shenzhen Stock Exchange stocks (e.g., 000001.SZ for Ping An Bank)

## Usage

```bash
cargo run --bin china_stock -- --api-key YOUR_API_KEY --secret-key YOUR_SECRET_KEY --environment sim
```

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
- Volume and turnover data
- Timestamp information

## Implementation Details

This is a mock implementation designed for development and testing. It does not connect to real Chinese stock exchanges but provides the same interface structure you would find in a production system. To connect to real exchanges, you would need to implement the actual API calls to providers like Hithink RoyalFlush, WeBank, or other licensed Chinese financial data providers.