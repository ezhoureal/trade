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
Single pair mode (defaults to cu vs fu if not supplied):
```
./target/release/rust_strategy --data ./data --commodity-a cu --commodity-b fu
```

Multiple pairs (inline comma list using `a:b,c:d,...`):
```
./target/release/rust_strategy --data ./data --pairs cu:fu,rb:hc,au:ag
```

Multiple pairs from a file (one `a:b` per line, `#` comments allowed):
```
cat > pairs.txt <<EOF
cu:fu
rb:hc
au:ag
# comment lines ignored
EOF
./target/release/rust_strategy --data ./data --pairs-file pairs.txt
```

You can combine `--pairs` and `--pairs-file`; duplicates are de-duplicated preserving first occurrence order.

## Differences vs Python Version
- Portfolio P&L currently emulates the Python logic using z-score differential rather than mark-to-market; can be extended.
- No Backtrader; custom discrete event loop over dates.
- Expiration assumption: midpoint (15th) of contract month.
