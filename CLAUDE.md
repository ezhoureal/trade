# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a commodity futures pairs trading system with both backtesting and live trading capabilities. The project implements statistical arbitrage strategies on Chinese commodity futures markets, focusing on cointegrated pairs trading.

**Key Pipeline (from pipeline.md):**
1. Excel → Parquet conversion
2. Calculate co-integration for every pair of contracts
3. Select valid cointegrated pairs
4. Backtest statistical arbitrage strategy on all valid pairs
5. Select top performers for live trading

## Architecture

### Hybrid Language Structure

The codebase uses **Rust for performance-critical components** and **Python for data acquisition and analysis**:

- **Rust workspace** (`backtest/`, `ctp/`): Core backtesting engine and live trading via CTP (China Trading Platform) API
- **Python modules** (`data/`, `pair_discovery/`, `alpaca/`, `ibkr/`): Data crawling, pair discovery, and alternative broker integrations

### Rust Components

**`backtest/` crate:**
- Core pairs trading backtesting engine using Polars for data processing
- `engine.rs`: Simulation engine with position management and PnL tracking
- `strategy.rs`: PairStrategy implementation with z-score based entry/exit logic
- `params.rs`: Strategy parameters (lookback window, entry/exit thresholds, hedge ratios)
- `data.rs`: Market data loading and preprocessing
- Supports both single-pair and multi-pair parallel backtesting

**`ctp/` crate:**
- Live trading implementation using CTP API (ctp2rs bindings)
- `live_trade.rs`: Real-time trade execution
- `market_data.rs`: Live market data streaming
- `data_monitor.rs`: Market data monitoring and recording
- Supports two environments: Sim (仿真) and Tts (7x24 simulation)
- Depends on `backtest` crate with `live` feature enabled

### Python Components

**`data/` module:**
- `crawl.py`: Fetches futures data from AKShare for specified commodities
- `convert.py`: Excel to Parquet conversion
- `back_adjust.py`: Back-adjustment for continuous contract series
- `backtest/compute_commodity_averages.py`: Computes average prices for hedge ratio calculation

**`pair_discovery/` module:**
- `scan_pairs.py`: Cointegration testing for commodity pairs
- `run_all_pairs.py`: Batch processing of pairs trading strategies
- `market_utils.py`: Market data utilities

**`alpaca/` and `ibkr/` modules:**
- Alternative broker integrations for US markets (Alpaca) and Interactive Brokers

## Prerequisites

### System Dependencies

**For building Rust code:**
```bash
# Install C compiler and build tools
sudo apt update
sudo apt install build-essential

# Install libclang (required by ctp2rs for bindgen)
sudo apt install libclang-dev
```

### CTP Live Trading Requirements

To run the `ctp` binary for live trading, you need:

**1. Market Data File:**
- Required: `data/recent.parquet` (relative to project root)
- Create using the Python data crawler:
  ```bash
  uv run python data/crawl.py --commodity ag --days 30 --out data/recent.parquet
  ```

**2. CTP Dynamic Libraries:**
- Required for Linux: `ctp/api/lin64/thostmduserapi_se.so` and `ctp/api/lin64/thosttraderapi_se.so`
- These are proprietary libraries from CTP/OpenCTP provider. [Download resource](http://www.openctp.cn/TTS-CTPAPI.html).
- Different paths are used depending on environment:
  - **Tts environment** (default): `ctp/api/lin64/`. v6.7.2 Already included in the repo.
  - **Sim environment**: `../../../ctp-dyn/api/ctp/v6.7.2/...` (external path)

**3. OpenCTP Credentials:**
- Set environment variables:
  ```bash
  export OPENCTP_USER_ID="your_user_id"
  export OPENCTP_PASS="your_password"
  ```
- Or pass via command line arguments (see live trading commands below)

## Common Commands

### Rust Development

**Build all workspace members:**
```bash
cargo build --verbose
```

**Run tests:**
```bash
# Test backtest crate
cargo test -p backtest --verbose

# Test ctp crate
cargo test -p ctp --verbose
```

**Run backtest for a single pair:**
```bash
cargo run -p backtest -- -a cu -b fu --data data/backtest
```

**Run backtest with custom parameters:**
```bash
cargo run -p backtest -- \
  -a ag -b ni \
  --lookback-zscore 20 \
  --entry-z 2.0 \
  --exit-z 0.5 \
  --data data/backtest \
  --out results.json
```

**Run backtest for multiple pairs from file:**
```bash
cargo run -p backtest -- \
  --pairs-file data/commodity_pairs.txt \
  --data data/backtest \
  --out multi_pair_results.json
```

**Run live trading (CTP):**
```bash
# Requires OPENCTP_USER_ID and OPENCTP_PASS environment variables
cargo run -p ctp -- \
  --environment tts \
  --user-id $OPENCTP_USER_ID \
  --password $OPENCTP_PASS
```

**Build optimized release:**
```bash
cargo build --release
```

### Python Development

**Install dependencies (using uv):**
```bash
uv sync
```

**Crawl futures data:**
```bash
uv run python data/crawl.py --commodity ag --days 30 --out data/ag_recent.parquet
```

**Run pair discovery:**
```bash
uv run python pair_discovery/scan_pairs.py
```

**Run pairs trading backtest (Python version):**
```bash
uv run python pair_discovery/run_all_pairs.py
```

**Compute commodity average prices:**
```bash
uv run python data/backtest/compute_commodity_averages.py
```

## Key Strategy Parameters

The pairs trading strategy uses z-score based mean reversion:

- **Lookback window**: Default 20 days for z-score calculation
- **Entry threshold**: ±2.0σ (enter when spread diverges)
- **Exit threshold**: ±0.5σ (exit when spread reverts)
- **Hedge ratio**: Computed from average prices in `commodity_average_prices.json`
- **Expiry management**: Force-close positions 3 days before contract expiry

## Data Format

Market data is stored in Parquet format with schema:
- `Date`: Trading date
- `Contract`: Contract symbol (e.g., "ag2409")
- `Bar`: Sequential bar number for backtesting
- `Close`: Closing price
- `Volume`: Trading volume

The `commodity_average_prices.json` file in `data/backtest/` contains average prices per commodity for hedge ratio calculation.

## Top Performing Strategies (from pipeline.md)

Based on backtesting results:
1. **Base Metals Cross-Pair**: Cu-Fu (81.2% win rate), Al-Ni (Sharpe 48.6)
2. **Base Metals vs Energy**: 77% average win rate across 51 pairs
3. **Copper-Centric**: Cu appears in 92 profitable pairs
4. **Calendar Spreads**: Same commodity, different months (Ni2403-Ni2411)
5. **Steel Complex**: Rb-Hc, Ss-Wr (lower volatility, consistent returns)

Optimal parameters: 20-day window, ±2σ entry, ±0.5σ exit

## Development Notes

- The Rust workspace uses Polars for high-performance dataframe operations
- CTP integration requires platform-specific dynamic libraries (configured in `ctp/src/main.rs`)
- Python code uses AKShare for Chinese futures market data
- The backtest engine supports parallel execution via Rayon for multi-pair testing
- Live trading uses async/await with Tokio runtime
