import argparse
import csv
import concurrent.futures
import os

import pandas as pd
from statsmodels.tsa.stattools import coint

def build_contract_series(df: pd.DataFrame, price_col: str):
    """Build a dict mapping contract -> price Series indexed by Date."""
    if "Date" not in df.columns:
        raise KeyError("Dataframe missing 'Date' column")

    df = df.copy()
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

    # Quick validation: coint requires non-constant, finite input
    a_vals = merged["A"]
    b_vals = merged["B"]
    if a_vals.nunique() <= 1 or b_vals.nunique() <= 1:
        # skip flat series, not useful
        return None

    try:
        stat, pvalue, _ = coint(a_vals, b_vals)  # use named columns
        if pvalue < alpha:
            return {"A": a, "B": b, "pvalue": float(pvalue), "stat": float(stat), "nobs": len(merged)}
    except Exception as e:
        print(f'coint failed: {a}, {b}, {e}')
    return None

import glob
def scan(path: str, price_col: str,
         alpha: float, minobs: int, out: str, debug: bool):
    if os.path.isdir(path):
        parquet_files = glob.glob(os.path.join(path, "*.parquet"))
        dfs = []
        for file in sorted(parquet_files):
            dfs.append(pd.read_parquet(file))
        df = pd.concat(dfs, ignore_index=True)
    else:
        df = pd.read_parquet(path)

    # basic contract filtering (contracts like ABCD2025)
    contract_mask = df["Contract"].str.match(r"^[A-Za-z]+\d{4}$", na=False)
    df = df[contract_mask]

    if price_col not in df.columns:
        print(f'columns = {df.columns}')
        raise KeyError("Could not detect a price column (Settle/Close)")

    workers = os.cpu_count() or 1
    series_map = build_contract_series(df, price_col)
    contracts = sorted(series_map.keys())
    total_pairs = len(contracts) * (len(contracts) - 1) // 2
    print(f"Scanning {len(contracts)} contracts => {total_pairs} pairs using {workers or os.cpu_count()} workers")

    pairs = [(a, b) for i, a in enumerate(contracts) for b in contracts[i + 1 :]]

    # Prepare tasks (pass series objects to workers)
    tasks = [(a, b, series_map[a], series_map[b], minobs, alpha) for a, b in pairs]
    results = []
    if debug:
        for task in tasks:
            res = _pair_task(*task)
            results.append(res)
    else:
        # Use process pool for CPU-bound coint tests
        with concurrent.futures.ProcessPoolExecutor(max_workers=workers) as ex:
            # map with generator to avoid building huge list in memory when many pairs
            futures = [ex.submit(_pair_task, a, b, a_ser, b_ser, minobs, alpha) for a, b, a_ser, b_ser, minobs, alpha in tasks]
            for i, fut in enumerate(concurrent.futures.as_completed(futures), 1):
                res = fut.result()
                if res:
                    results.append(res)
                if i % 1000 == 0 or i == len(futures):
                    print(f"Processed {i}/{len(futures)} pairs, found {len(results)} significant so far")

    import math
    def _pval_key(r):
        pv = r.get("pvalue")
        return pv if pv is not None else math.inf
    results = sorted(results, key=_pval_key) if results else []
    print(f"Found {len(results)} significant pairs (p<{alpha})")

    keys = ["A", "B", "pvalue", "stat", "nobs"]
    with open(out, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=keys)
        writer.writeheader()
        for r in results:
            writer.writerow(r)
    print(f"Wrote results to {out}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Scan all contract pairs for cointegration")
    parser.add_argument("--dir", "-d", default="data/")
    parser.add_argument("--price", choices=["Settle", "Close"], default="Close")
    parser.add_argument("--alpha", type=float, default=0.05)
    parser.add_argument("--minobs", type=int, default=30)
    parser.add_argument("--out", "-o", type=str, default="data/pairs_coint.csv")
    parser.add_argument("--debug", action="store_true")

    args = parser.parse_args()
    scan(args.dir, price_col=args.price, alpha=args.alpha, minobs=args.minobs, out=args.out, debug=args.debug)
