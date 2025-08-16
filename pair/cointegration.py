import pandas as pd
import statsmodels.api as sm
from statsmodels.tsa.stattools import coint
from typing import Optional

from market_utils import load_market_data, align_contract_series

def main(filepath: str = "all_2024.11.xlsx", contract_code_a: Optional[str] = None, contract_code_b: Optional[str] = None, price_col_override: Optional[str] = None):
    df = load_market_data(filepath)

    print("Columns after normalization:", df.columns.tolist())
    print(df.head(3))

    if "Contract" not in df.columns:
        raise KeyError("Could not find a 'Contract' column after normalization. Columns: " + str(df.columns.tolist()))

    # filter potential title/header rows by simple contract code pattern
    contract_mask = df["Contract"].str.match(r"^[A-Za-z]+\d{4}$", na=False)
    df = df[contract_mask | df.index.to_series().apply(lambda x: False)]

    contracts = df["Contract"].dropna().unique()
    print("Found contract codes (sample):", contracts[:10])

    if len(contracts) < 2:
        raise ValueError("Need at least two distinct contracts to run cointegration")

    if contract_code_a is None:
        contract_code_a = contracts[0]
    if contract_code_b is None:
        contract_code_b = next((c for c in contracts if c != contract_code_a), None)
    if contract_code_b is None:
        raise ValueError("Could not determine a second contract to compare")

    price_col = price_col_override if price_col_override else ("Settle" if "Settle" in df.columns else ("Close" if "Close" in df.columns else None))
    if price_col is None:
        raise KeyError("Neither 'Settle' nor 'Close' column found after normalization. Columns: " + str(df.columns.tolist()))

    print(f"Using price column: {price_col}")

    aligned = align_contract_series(df, contract_code_a, contract_code_b, price_col)
    if not aligned:
        print(f"No overlapping trading dates found for {contract_code_a} and {contract_code_b}; aborting test.")
        return
    contractA, contractB = aligned

    score, pvalue, _ = coint(contractA, contractB)
    print("====== Cointegration Test Results:")
    print(f"Test statistic: {score}")
    print(f"p-value: {pvalue}")

    if pvalue < 0.05:
        print("✅ The two contracts are cointegrated (5% significance).")
    else:
        print("❌ No strong evidence of cointegration.")

    X = sm.add_constant(contractB)
    model = sm.OLS(contractA, X).fit()
    hedge_ratio = model.params.iloc[1] if hasattr(model.params, 'iloc') else model.params[1]
    print(f"Estimated hedge ratio (beta): {hedge_ratio}")


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Run cointegration test between two contracts in an Excel report")
    parser.add_argument("--file", "-f", default="all_2024.11.xlsx", help="Excel file to load")
    parser.add_argument("--a", help="Contract code for series A (e.g. ag2412)")
    parser.add_argument("--b", help="Contract code for series B (e.g. ag2413)")
    parser.add_argument("--price", choices=["Settle", "Close"], help="Price column to use (Settle or Close)")

    args = parser.parse_args()

    main(filepath=args.file, contract_code_a=args.a, contract_code_b=args.b, price_col_override=args.price)
