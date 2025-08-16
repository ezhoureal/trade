import argparse
import csv
from itertools import combinations

import pandas as pd
from statsmodels.tsa.stattools import coint

from backtrader.market_utils import load_market_data, align_contract_series
def scan(filepath: str, price_col: str = None, alpha: float = 0.05, minobs: int = 30, out: str = None):
    df = load_market_data(filepath)

    # basic contract filtering
    contract_mask = df["Contract"].str.match(r"^[A-Za-z]+\d{4}$", na=False)
    df = df[contract_mask]

    if price_col is None:
        price_col = "Settle" if "Settle" in df.columns else ("Close" if "Close" in df.columns else None)
    if price_col is None:
        raise KeyError("Could not detect a price column (Settle/Close)")

    contracts = sorted(df["Contract"].dropna().unique())
    print(f"Scanning {len(contracts)} contracts => {len(contracts)*(len(contracts)-1)//2} pairs")

    results = []
    for a, b in combinations(contracts, 2):
        aligned = align_contract_series(df, a, b, price_col)
        if not aligned:
            continue
        a_ser, b_ser = aligned
        if len(a_ser) < minobs:
            continue
        try:
            stat, pvalue, _ = coint(a_ser, b_ser)
        except Exception:
            continue
        if pvalue < alpha:
            results.append({"A": a, "B": b, "pvalue": pvalue, "stat": stat, "nobs": len(a_ser)})

    results = sorted(results, key=lambda r: r["pvalue"]) if results else []
    print(f"Found {len(results)} significant pairs (p<{alpha})")
    for r in results:
        print(r)

    if out and results:
        keys = ["A", "B", "pvalue", "stat", "nobs"]
        with open(out, "w", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=keys)
            writer.writeheader()
            for r in results:
                writer.writerow(r)
        print(f"Wrote results to {out}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Scan all contract pairs for cointegration")
    parser.add_argument("--file", "-f", default="all_2024.11.xlsx")
    parser.add_argument("--price", choices=["Settle", "Close"], help="Price column to use")
    parser.add_argument("--alpha", type=float, default=0.05)
    parser.add_argument("--minobs", type=int, default=30)
    parser.add_argument("--out", help="CSV output file")

    args = parser.parse_args()
    scan(args.file, price_col=args.price, alpha=args.alpha, minobs=args.minobs, out=args.out)
