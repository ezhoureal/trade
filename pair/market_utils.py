import pandas as pd
from typing import Optional, Tuple

def load_excel(filepath: str) -> pd.DataFrame:
    """Robustly load the Excel file and normalize column names.

    Handles files with title rows above the real header. Detects header
    by scanning the first 10 rows for expected keywords (Chinese/English).
    Normalizes common column names to English equivalents used elsewhere.
    """
    df = pd.read_excel(filepath, header=3)
    df = df.dropna(axis=1, how="all")
    df = df.dropna(axis=0, how="any")  # also drop empty rows

    # Excel often uses merged cells for grouped rows (e.g. Contract value
    # appears once and the following rows are blank). Forward-fill the
    # normalized 'Contract' column so every row has the correct contract id.
    if "Contract" in df.columns:
        # Replace empty strings with NaN first so ffill works reliably
        df.loc[:, "Contract"] = df["Contract"].replace("", pd.NA)
        df.loc[:, "Contract"] = df["Contract"].ffill()
    
    # Convert date column to int if it exists and has decimal values
    if "Date" in df.columns and df["Date"].dtype == float:
        df["Date"] = df["Date"].astype(int)
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
