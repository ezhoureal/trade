# Rust Port of Dynamic Copper-Fuel Oil Pairs Strategy

This is a Rust reimplementation of the Python Backtrader strategy `dynamic_cu_fu_pairs.py`.

## Goals
- Load multiple Parquet files of contract data (copper `cu` and fuel `fu`).
- Align trading days and construct per-contract aligned time series.
- Maintain rolling spread histories for all cu/fu contract pairs.
- Enter/exit mean-reversion spread positions based on z-score thresholds.
- Track per-pair performance (returns, Sharpe-like ratio, success score).
- Output summary metrics and trade log as JSON.

## Status
Initial scaffolding. Engine logic in progress.

## Data Requirements
Columns expected (case-sensitive):
```
Date (YYYYMMDD or ISO 8601 acceptable)
Contract (e.g. cu2405, fu2501)
Open High Low Close Volume OI
```

## Build
```
cargo build --release
```

## Run (example)
```
./target/release/rust_strategy --data ./data --entry-z 2.0 --exit-z 0.5 --lookback-zscore 20 --max-pairs 3 --eval-freq 10
```

## Differences vs Python Version
- Portfolio P&L currently emulates the Python logic using z-score differential rather than mark-to-market; can be extended.
- No Backtrader; custom discrete event loop over dates.
- Expiration assumption: midpoint (15th) of contract month.

## Next Steps
- Implement data loader (Polars) with alignment.
- Implement pair selector & stats.
- Implement engine loop and JSON output.
