import pandas as pd
import statsmodels.api as sm
from statsmodels.tsa.stattools import coint

# === Step 1: Load Data from Excel ===
# Replace with your Excel filename
file = "all_2024.11.xlsx"
df = pd.read_excel(file)
print(df[:3])

# Example: compare two contracts by 'Close' or 'Settle'
# You may need to filter by contract code if multiple exist in one file

# Let's say Contract A = first contract, Contract B = second contract
# Adjust depending on how your Excel is structured
import pandas as pd
import statsmodels.api as sm
from statsmodels.tsa.stattools import coint
from typing import Optional


def load_market_data(filepath: str) -> pd.DataFrame:
    """Robustly load the Excel file and normalize column names.

    This handles files that include a title row above the real header
    (common in reports). It detects a header row by searching the first
    10 rows for expected keywords (Chinese/English). It then normalizes
    common column names to English equivalents used by the script.
    """
    # peek first rows to find header line
    preview = pd.read_excel(filepath, header=None, nrows=10)

    keywords = [
        "合约",
        "Contract",
        "结算",
        "结算价",
        "Settle",
        "收盘",
        "收盘价",
        "交易日期",
        "Date",
    ]

    # pick the last preview row that contains any keyword - handles multi-row headers
    header_row = None
    for i, row in preview.iterrows():
        cells = [str(x).strip() for x in row.values if pd.notna(x)]
        if not cells:
            continue
        if any(any(k in cell for cell in cells) for k in keywords):
            header_row = i

    if header_row is None:
        df = pd.read_excel(filepath)
    else:
        df = pd.read_excel(filepath, header=header_row)

    # drop entirely empty columns
    df = df.dropna(axis=1, how="all")

    # mapping of possible Chinese/verbose names to normalized English names
    rename_map = {
        "合约": "Contract",
        "所内合约行情报表": "Contract",
        "Contract": "Contract",
        "交易日期": "Date",
        "Date": "Date",
        "结算价": "Settle",
        "结算": "Settle",
        "Settle": "Settle",
        "收盘价": "Close",
        "收盘": "Close",
        "前收盘": "pre close",
        "前结算": "Pre settle",
        "持仓量": "OI",
        "成交量": "Volume",
        "成交金额(万元)": "Amount",
    }

    new_cols = {}
    for c in df.columns:
        cstr = str(c).strip()
        if cstr in rename_map:
            new_cols[c] = rename_map[cstr]
        else:
            for k, v in rename_map.items():
                if k in cstr:
                    new_cols[c] = v
                    break

    if new_cols:
        df = df.rename(columns=new_cols)

    # strip whitespace in object columns
    for col in df.select_dtypes(include=[object]).columns:
        df[col] = df[col].astype(str).str.strip()

    return df

def main(filepath: str = "all_2024.11.xlsx", contract_code_a: Optional[str] = None, contract_code_b: Optional[str] = None, price_col_override: Optional[str] = None):
    file = filepath
    df = load_market_data(file)

    print("Columns after normalization:", df.columns.tolist())
    print(df.head(3))

    if "Contract" not in df.columns:
        raise KeyError("Could not find a 'Contract' column after normalization. Columns: " + str(df.columns.tolist()))

    # drop rows that are leftover header rows or title rows by filtering Contract codes
    # assumption: valid contract codes are letters followed by 4 digits, e.g. ag2412
    contract_mask = df["Contract"].str.match(r"^[A-Za-z]+\d{4}$", na=False)
    if not contract_mask.any():
        # as a fallback, keep rows where the suspected price columns are numeric
        print("No contract-like rows found with pattern; will attempt to detect data rows by numeric price columns")

    df = df[contract_mask | df.index.to_series().apply(lambda x: False)]

    contracts = df["Contract"].dropna().unique()
    print("Found contract codes (sample):", contracts[:10])

    if len(contracts) < 2:
        raise ValueError("Need at least two distinct contracts to run cointegration")

    if contract_code_a is None:
        contract_code_a = contracts[0]
    if contract_code_b is None:
        # pick next distinct contract
        contract_code_b = next((c for c in contracts if c != contract_code_a), None)
    if contract_code_b is None:
        raise ValueError("Could not determine a second contract to compare")

    price_col = price_col_override if price_col_override else ("Settle" if "Settle" in df.columns else ("Close" if "Close" in df.columns else None))
    if price_col is None:
        raise KeyError("Neither 'Settle' nor 'Close' column found after normalization. Columns: " + str(df.columns.tolist()))

    print(f"Using price column: {price_col}")

    # coerce price column to numeric and drop rows where not numeric
    df[price_col] = pd.to_numeric(df[price_col], errors="coerce")
    df = df.dropna(subset=[price_col, "Contract"])  # require both

    contractA = df[df["Contract"] == contract_code_a][price_col].reset_index(drop=True)
    contractB = df[df["Contract"] == contract_code_b][price_col].reset_index(drop=True)

    # align lengths
    min_len = min(len(contractA), len(contractB))
    contractA = contractA[:min_len]
    contractB = contractB[:min_len]

    # cointegration test
    score, pvalue, _ = coint(contractA, contractB)
    print("Cointegration Test Results:")
    print(f"Test statistic: {score}")
    print(f"p-value: {pvalue}")

    if pvalue < 0.05:
        print("✅ The two contracts are cointegrated (5% significance).")
    else:
        print("❌ No strong evidence of cointegration.")

    # OLS hedge ratio
    X = sm.add_constant(contractB)
    model = sm.OLS(contractA, X).fit()
    hedge_ratio = model.params[1]
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
