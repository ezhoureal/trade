import argparse
import csv
import concurrent.futures
import os

import pandas as pd
from statsmodels.tsa.stattools import coint

from market_utils import load_parquet


def build_contract_series(df: pd.DataFrame, price_col: str):
    """Build a dict mapping contract -> price Series indexed by Date."""
    if "Date" not in df.columns:
        raise KeyError("Dataframe missing 'Date' column")

    df = df.copy()
    df[price_col] = pd.to_numeric(df[price_col], errors="coerce")
    df["Date"] = pd.to_datetime(df["Date"], errors="coerce")
    df = df.dropna(subset=[price_col, "Contract", "Date"])

    series_map = {}
    for contract, group in df.groupby("Contract"):
        s = group.set_index("Date")[price_col].sort_index()
        if not s.empty:
            series_map[contract] = s
    return series_map


def _pair_task(a: str, b: str, a_ser: pd.Series, b_ser: pd.Series, minobs: int, alpha: float):
    """Worker task: merge two series, run coint, return result dict or None."""
    # Ensure columns have distinct names to avoid overlap during join
    a_df = a_ser.rename("A").to_frame()
    b_df = b_ser.rename("B").to_frame()
    merged = a_df.join(b_df, how="inner")
    if merged.empty or len(merged) < minobs:
        return None
    stat, pvalue, _ = coint(merged["A"], merged["B"])  # use named columns
    if pvalue < alpha:
        return {"A": a, "B": b, "pvalue": float(pvalue), "stat": float(stat), "nobs": len(merged)}
    return None

def scan(filepath: str = "data/combined.parquet", price_col: str = None,
         alpha: float = 0.05, minobs: int = 30):
    df = load_parquet(filepath)

    # basic contract filtering (contracts like ABCD2025)
    contract_mask = df["Contract"].str.match(r"^[A-Za-z]+\d{4}$", na=False)
    df = df[contract_mask]

    if price_col is None:
        price_col = "Settle" if "Settle" in df.columns else ("Close" if "Close" in df.columns else None)
    if price_col is None:
        raise KeyError("Could not detect a price column (Settle/Close)")

    workers = os.cpu_count() or 1
    series_map = build_contract_series(df, price_col)
    contracts = sorted(series_map.keys())
    total_pairs = len(contracts) * (len(contracts) - 1) // 2
    print(f"Scanning {len(contracts)} contracts => {total_pairs} pairs using {workers or os.cpu_count()} workers")

    pairs = [(a, b) for i, a in enumerate(contracts) for b in contracts[i + 1 :]]

    # Prepare tasks (pass series objects to workers)
    tasks = [(a, b, series_map[a], series_map[b], minobs, alpha) for a, b in pairs]
    tasks = tasks[90000:]
    results = []
    for task in tasks:
        _pair_task(*task)
    # Use process pool for CPU-bound coint tests
    # with concurrent.futures.ProcessPoolExecutor(max_workers=workers) as ex:
    #     # map with generator to avoid building huge list in memory when many pairs
    #     futures = [ex.submit(_pair_task, a, b, a_ser, b_ser, minobs, alpha) for a, b, a_ser, b_ser, minobs, alpha in tasks]
    #     for i, fut in enumerate(concurrent.futures.as_completed(futures), 1):
    #         res = fut.result()
    #         if res:
    #             results.append(res)
    #         if i % 1000 == 0 or i == len(futures):
    #             print(f"Processed {i}/{len(futures)} pairs, found {len(results)} significant so far")

    import math
    def _pval_key(r):
        pv = r.get("pvalue")
        return pv if pv is not None else math.inf
    results = sorted(results, key=_pval_key) if results else []
    print(f"Found {len(results)} significant pairs (p<{alpha})")

    if results:
        keys = ["A", "B", "pvalue", "stat", "nobs"]
        with open("data/pairs_coint.csv", "w", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=keys)
            writer.writeheader()
            for r in results:
                writer.writerow(r)
        print(f"Wrote results to data/pairs_coint.csv")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Scan all contract pairs for cointegration")
    parser.add_argument("--file", "-f", default="data/combined.parquet")
    parser.add_argument("--price", choices=["Settle", "Close"], default=None)
    parser.add_argument("--alpha", type=float, default=0.05)
    parser.add_argument("--minobs", type=int, default=5)

    args = parser.parse_args()
    scan(args.file, price_col=args.price, alpha=args.alpha, minobs=args.minobs)
