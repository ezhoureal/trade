import pandas as pd
from pathlib import Path
from typing import Optional, Tuple

def detect_excel_header(filepath: str, nrows: int = 10) -> Optional[int]:
    """Scan the first `nrows` of an Excel file and return the header row index
    if a likely header is found (based on keywords), otherwise return None.
    """
    preview = pd.read_excel(filepath, header=None, nrows=nrows)

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

    header_row: Optional[int] = None
    for i, row in preview.iterrows():
        cells = [str(x).strip() for x in row.values if pd.notna(x)]
        if not cells:
            continue
        if any(any(k in cell for cell in cells) for k in keywords):
            header_row = i
            break

    return header_row

def load_parquet(filepath: str = "data/combined.parquet") -> pd.DataFrame:
    """Load a Parquet file into a DataFrame."""
    return pd.read_parquet(filepath)

def load_excel(filepath: str) -> pd.DataFrame:
    """Robustly load the Excel file and normalize column names.

    Handles files with title rows above the real header. Detects header
    by scanning the first 10 rows for expected keywords (Chinese/English).
    Normalizes common column names to English equivalents used elsewhere.
    """
    header_row = detect_excel_header(filepath, nrows=10)
    if header_row is None:
        df = pd.read_excel(filepath)
    else:
        df = pd.read_excel(filepath, header=header_row)

    df = df.dropna(axis=1, how="all")

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

    # Excel often uses merged cells for grouped rows (e.g. Contract value
    # appears once and the following rows are blank). Forward-fill the
    # normalized 'Contract' column so every row has the correct contract id.
    if "Contract" in df.columns:
        # Replace empty strings with NaN first so ffill works reliably
        df.loc[:, "Contract"] = df["Contract"].replace("", pd.NA)
        df.loc[:, "Contract"] = df["Contract"].ffill()

    # Strip whitespace from object columns (do this after ffill so NaNs
    # are preserved until conversion to string)
    for col in df.select_dtypes(include=[object]).columns:
        df[col] = df[col].astype(str).str.strip()

    return df


def align_contract_series(df: pd.DataFrame, a_code: str, b_code: str, price_col: str) -> Optional[Tuple[pd.Series, pd.Series]]:
    """Return (A_series, B_series) aligned by Date, or None if no overlap.

    Parses Date to datetime; inner-joins on Date. Returns None when Date
    is missing or there are no overlapping dates.
    """
    df = df.copy()
    df[price_col] = pd.to_numeric(df[price_col], errors="coerce")
    df = df.dropna(subset=[price_col, "Contract"])  # require both

    if "Date" not in df.columns:
        return None

    a_df = df[df["Contract"] == a_code][["Date", price_col]].copy()
    b_df = df[df["Contract"] == b_code][["Date", price_col]].copy()

    a_df["Date"] = pd.to_datetime(a_df["Date"], errors="coerce")
    b_df["Date"] = pd.to_datetime(b_df["Date"], errors="coerce")

    a_df = a_df.dropna(subset=["Date", price_col])
    b_df = b_df.dropna(subset=["Date", price_col])

    a_series = a_df.set_index("Date")[price_col].rename("A")
    b_series = b_df.set_index("Date")[price_col].rename("B")

    merged = a_series.to_frame().join(b_series.to_frame(), how="inner")
    if merged.empty:
        return None

    return merged["A"], merged["B"]
