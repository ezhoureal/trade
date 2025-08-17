"""Convert SHFE trading Excel files to Parquet
Usage:
    python -m data.excel_to_parquet --year 2024
"""
from __future__ import annotations

import argparse
import logging
from pathlib import Path
from typing import List

import pandas as pd

from pair.market_utils import load_excel

logger = logging.getLogger("excel_to_parquet")

def concat_excels(src_dir: Path, year: int) -> pd.DataFrame:
    files = sorted(src_dir.glob(f"*{year}*.xls*"))
    if not files:
        return pd.DataFrame()

    dfs = []
    for f in files:
        logger.info("Loading %s", f)
        df = load_excel(str(f))
        dfs.append(df)

    combined = pd.concat(dfs, ignore_index=True, sort=False)
    print(f'{combined}')
    return combined


def write_parquet(df: pd.DataFrame, out_path: str) -> None:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    print(f'df columns = {df.columns}')
    try:
        df.to_parquet(out_path, index=False, engine="pyarrow", compression="snappy")
        logger.info("Wrote parquet to %s", out_path)
    except Exception as e:
        logger.exception("Failed to write parquet to %s: %s", out_path, e)
        raise


def main(argv: List[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--year", "-y", type=int, default=2024)
    parser.add_argument("--dir", "-d", default="data/", help="Source directory containing Excel files")
    args = parser.parse_args(argv)

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

    src = Path(args.dir)
    combined = concat_excels(src, year=args.year)
    write_parquet(combined, out_path=Path(f'data/{args.year}.parquet'))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
