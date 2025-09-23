"""Futures data crawler

Fetch the last `look_back_period` trading days for a given commodity prefix
from AKShare and persist a normalized dataset (Date, Contract, Price, Volume)
to a parquet file.

Usage (from project root):
	uv run python data/crawl.py --commodity ag --days 30 --out data/ag_recent.parquet

Notes:
 - AKShare endpoints vary by exchange; here we use the generic daily futures
   market data endpoint per contract. For SHFE style symbols (e.g. ag2409), we
   introspect active contracts first, then pull daily history for each and
   filter to last N days.
 - Adjust or extend with DCE/CZCE endpoints as needed.
"""

from __future__ import annotations

import argparse
import datetime as dt
import sys
from pathlib import Path
from typing import List

import pandas as pd
import akshare as ak


def _parse_args() -> argparse.Namespace:
	p = argparse.ArgumentParser()
	p.add_argument("--commodity", default="ag", help="Commodity prefix, e.g. ag")
	p.add_argument(
		"--days", "--look-back-period", dest="days", type=int, default=30, help="Look back trading days"
	)
	p.add_argument("--out", type=Path, default="recent.parquet", help="Output parquet path")
	p.add_argument(
		"--exchange",
		default="shfe",
		choices=["shfe"],
		help="Exchange code (currently only shfe wired)",
	)
	return p.parse_args()


def _list_active_contracts_shfe(prefix: str) -> List[str]:
	# AKShare provides contract info by date (YYYYMMDD). Use today's date.
	today = dt.date.today().strftime("%Y%m%d")
	try:
		df = ak.futures_contract_info_shfe(date=today)
	except Exception as e:  # network or API issues
		print(f"Error fetching contract info: {e}", file=sys.stderr)
		return []
	print(f'columns are {df.columns}')
	if df is None or df.empty:
		return []
	candidates = df["合约代码"].astype(str).tolist()
	# Filter by prefix (case-insensitive) and simple length guard (prefix + digits)
	prefix_lower = prefix.lower()
	out = []
	for s in candidates:
		s2 = s.lower()
		if s2.startswith(prefix_lower):
			out.append(s)
	return sorted(set(out))


def _fetch_daily_contract_df(contract: str) -> pd.DataFrame:
	"""Return daily OHLCV for a single contract with Date column as datetime.date.

	AKShare has multiple futures daily endpoints; here we attempt a generic one.
	Fallback strategy: try the continuous daily API if specific fails.
	"""
	# Primary attempt: exchange-specific daily. For SHFE, use futures_daily.
	try:
		raw = ak.futures_zh_daily_sina(contract)
	except Exception:
		raw = None
	if raw is None or raw.empty:
		return pd.DataFrame()
	# Normalize columns; AKShare typical columns: 'date','open','high','low','close','volume'
	cols = {c.lower(): c for c in raw.columns}
	date_col = next((cols[k] for k in ["date", "tradedate"] if k in cols), None)
	close_col = next((cols[k] for k in ["close", "settlement", "closingprice"] if k in cols), None)
	volume_col = next((cols[k] for k in ["volume", "vol"] if k in cols), None)
	if not (date_col and close_col and volume_col):
		return pd.DataFrame()
	df = raw[[date_col, close_col, volume_col]].copy()
	df.columns = ["Date", "Price", "Volume"]
	# Parse date
	df["Date"] = pd.to_datetime(df["Date"]).dt.date
	df["Contract"] = contract
	return df


def get_data(commodity: str, look_back_period: int) -> pd.DataFrame:
	contracts = _list_active_contracts_shfe(commodity)
	if not contracts:
		raise SystemExit("No contracts found for prefix")
	print(f'Found {len(contracts)} contracts for prefix {commodity}: {contracts}')
	frames = []
	for c in contracts:
		df_c = _fetch_daily_contract_df(c)
		if df_c.empty:
			continue
		frames.append(df_c)
	if not frames:
		raise SystemExit("No historical data retrieved")
	df_all = pd.concat(frames, ignore_index=True)
	# Keep only last N unique trading dates globally (not per contract)
	unique_dates = sorted(df_all["Date"].unique())
	if len(unique_dates) > look_back_period:
		cutoff_dates = set(unique_dates[-look_back_period:])
		df_all = df_all[df_all["Date"].isin(cutoff_dates)]
	# Sort for determinism
	df_all = df_all.sort_values(["Date", "Contract"]).reset_index(drop=True)
	return df_all[["Date", "Contract", "Price", "Volume"]]


def main():
	args = _parse_args()
	df = get_data(args.commodity, args.days)
	args.out.parent.mkdir(parents=True, exist_ok=True)
	df.to_parquet(args.out, index=False)
	print(
		f"Wrote {len(df)} rows across {df['Contract'].nunique()} contracts to {args.out} (dates {df['Date'].min()} .. {df['Date'].max()})"
	)

if __name__ == "__main__":
	main()