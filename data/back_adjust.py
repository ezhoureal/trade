import pandas as pd
import re

# ---- Step 1: Parse commodity + expiry from contract ----
def parse_contract(contract):
    # e.g. "ag2401" → commodity="ag", expiry="2024-01"
    m = re.match(r"([a-zA-Z]+)(\d{2})(\d{2})", contract)
    if not m:
        return None, None
    commodity, yy, mm = m.groups()
    year = 2000 + int(yy) if int(yy) < 50 else 1900 + int(yy)  # crude Y2K handling
    expiry = pd.to_datetime(f"{year}-{mm}-01") + pd.offsets.MonthEnd(0)
    return commodity, expiry

# ---- Step 2: Preprocess raw data ----
def preprocess(df):
    df = df.copy()
    df["Date"] = pd.to_datetime(df["Date"])
    df[["commodity", "expiry"]] = df["Contract"].apply(
        lambda x: pd.Series(parse_contract(x))
    )
    return df.sort_values(["commodity", "Date"])

import pandas as pd
import argparse

def build_liquidity_roll(df, price_col="Close"):
    all_front = []
    for commodity, sub in df.groupby("commodity"):
        sub = sub.sort_values(["Date", "Contract"])
        
        # Step 1: pick contract with max OI each day
        idx = sub.groupby("Date")["OI"].idxmax()
        front = sub.loc[idx].sort_values("Date").reset_index(drop=True)
        
        # Step 2: back-adjust prices when rolling
        adjustment = 0.0
        prev_contract = None
        adj_prices = []
        
        for i, row in front.iterrows():
            if prev_contract is not None and row["Contract"] != prev_contract["Contract"]:
                # roll event → compute adjustment
                last_price = prev_contract[price_col]
                new_price = row[price_col]
                adjustment += (last_price - new_price)
            
            adj_price = row[price_col] + adjustment
            adj_prices.append(adj_price)
            
            prev_contract = row
        
        front[price_col] = adj_prices
        all_front.append(front)

    return pd.concat(all_front, ignore_index=True)[["Date", "Contract", price_col, "OI", "Volume"]]

# Command line argument parsing
parser = argparse.ArgumentParser(description='Back-adjust commodity futures data')
parser.add_argument('--input', '-i', required=True, help='Input parquet file path')
parser.add_argument('--output', '-o', required=True, help='Output parquet file path')
parser.add_argument('--price-col', default='Close', help='Price column to adjust (default: Close)')
args = parser.parse_args()

# Use command line arguments
file_name = args.input
df = pd.read_parquet(file_name)
df = preprocess(df)

continuous_data = build_liquidity_roll(df, price_col=args.price_col)

# Save to output file
continuous_data.to_parquet(args.output, index=False)
print(f"Back-adjusted data saved to {args.output}")
