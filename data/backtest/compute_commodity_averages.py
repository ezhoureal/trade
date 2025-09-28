"""
Compute average Close price per commodity across all parquet files in data/backtest/.

Commodity code is inferred as the leading alphabetic prefix of the Contract string,
case-insensitive (e.g., 'au2206' -> 'au').

Outputs JSON: data/commodity_average_prices.json mapping { commodity: average_close }.
"""

from __future__ import annotations

import json
import math
import os
import re
import sys
from collections import defaultdict
from glob import glob
from typing import Dict, Tuple

import pandas as pd


HERE = os.path.dirname(__file__)
BACKTEST_DIR = os.path.join(HERE, "backtest")
OUTPUT_PATH = os.path.join(HERE, "commodity_average_prices.json")


def extract_commodity(contract: str) -> str | None:
    if not isinstance(contract, str):
        return None
    m = re.match(r"^[A-Za-z]+", contract)
    if not m:
        return None
    return m.group(0).lower()


def aggregate_file(path: str) -> Dict[str, Tuple[float, int]]:
    """Return per-commodity (sum_close, count) from a single parquet file."""
    # Read only the needed columns to save memory/time
    df = pd.read_parquet(path, columns=["Contract", "Close"])  # type: ignore[arg-type]
    # Drop rows with missing values quickly
    df = df.dropna(subset=["Contract", "Close"])  # type: ignore[assignment]

    # Ensure types and filter out non-finite numbers
    # Convert Close to numeric (coerce errors to NaN then drop)
    df["Close"] = pd.to_numeric(df["Close"], errors="coerce")
    df = df[~df["Close"].isna()]

    # Extract commodity prefix
    commodities = df["Contract"].astype(str).map(extract_commodity)
    df = df.assign(_comm=commodities)
    df = df[df["_comm"].notna()]

    # Group by commodity and compute sum & count
    gb = df.groupby("_comm")["Close"].agg(["sum", "count"])  # type: ignore[index]
    # Convert to dict of commodity -> (sum, count)
    # Keys from groupby can be Hashable; cast to str for type clarity
    out: Dict[str, Tuple[float, int]] = {
        str(k): (float(v["sum"]), int(v["count"])) for k, v in gb.to_dict(orient="index").items()
    }
    return out


def main(output_path: str = OUTPUT_PATH) -> int:
    parquet_paths = sorted(glob(os.path.join(BACKTEST_DIR, "*.parquet")))
    if not parquet_paths:
        print(f"No parquet files found under {BACKTEST_DIR}", file=sys.stderr)
        return 1

    totals: Dict[str, float] = defaultdict(float)
    counts: Dict[str, int] = defaultdict(int)

    for p in parquet_paths:
        try:
            partial = aggregate_file(p)
        except Exception as e:
            print(f"Warning: failed to process {p}: {e}", file=sys.stderr)
            continue
        for comm, (s, c) in partial.items():
            if not math.isfinite(s) or c <= 0:
                continue
            totals[comm] += s
            counts[comm] += c

    # Compute averages
    averages: Dict[str, float] = {}
    for comm, s in totals.items():
        c = counts.get(comm, 0)
        if c > 0:
            averages[comm] = s / c

    if not averages:
        print("No averages computed (empty data?)", file=sys.stderr)
        return 2

    # Sort by commodity key for determinism
    averages_sorted = {k: averages[k] for k in sorted(averages.keys())}

    # Write JSON
    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    with open(output_path, "w", encoding="utf-8") as f:
        json.dump(averages_sorted, f, ensure_ascii=False, indent=2, sort_keys=False)

    print(f"Wrote {len(averages_sorted)} commodities to {output_path}")
    # Optional: print a couple of sample entries if present
    for key in ("au", "fu"):
        if key in averages_sorted:
            print(f"Sample {key}: {averages_sorted[key]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
