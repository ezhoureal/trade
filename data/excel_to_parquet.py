"""Convert all Excel files in a directory to a single Parquet by using
`pair.market_utils.load_market_data` for robust parsing and normalization.

Usage:
    python data/excel_to_parquet.py --dir data --out data/combined.parquet
"""
from __future__ import annotations

import argparse
import logging
from pathlib import Path
from typing import List

import pandas as pd

from pair.market_utils import load_market_data


logger = logging.getLogger("excel_to_parquet")


def find_excel_files(src_dir: Path, pattern: str = "*.xls*") -> List[Path]:
    return sorted(src_dir.glob(pattern))


def concat_excels(src_dir: Path, pattern: str = "*.xls*", add_source: bool = True) -> pd.DataFrame:
    files = find_excel_files(src_dir, pattern=pattern)
    if not files:
        logger.warning("No files found in %s matching %s", src_dir, pattern)
        return pd.DataFrame()

    dfs = []
    for f in files:
        logger.info("Loading %s", f)
        try:
            df = load_market_data(str(f))
        except Exception as e:  # keep robust: log and continue
            logger.exception("Failed to load %s: %s", f, e)
            continue

        if df.empty:
            logger.info("File %s produced empty DataFrame, skipping", f)
            continue

        if add_source:
            df = df.copy()
            df["_source_file"] = f.name

        dfs.append(df)

    if not dfs:
        logger.warning("No valid DataFrames produced from files in %s", src_dir)
        return pd.DataFrame()

    combined = pd.concat(dfs, ignore_index=True, sort=False)
    print(f'top 20 = {combined.head(20)}')
    return combined


def write_parquet(df: pd.DataFrame, out_path: Path, partition_cols: List[str] | None = None) -> None:
    out_path.parent.mkdir(parents=True, exist_ok=True)
    try:
        if partition_cols:
            df.to_parquet(out_path, index=False, engine="pyarrow", compression="snappy", partition_cols=partition_cols)
        else:
            df.to_parquet(out_path, index=False, engine="pyarrow", compression="snappy")
        logger.info("Wrote parquet to %s", out_path)
    except Exception as e:
        logger.exception("Failed to write parquet to %s: %s", out_path, e)
        raise


def main(argv: List[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", "-d", required=True, help="Source directory containing Excel files")
    parser.add_argument("--out", "-o", default="data/combined.parquet", help="Output Parquet file path")
    parser.add_argument("--pattern", default="*.xls*", help="Glob pattern for Excel files")
    parser.add_argument("--no-source-col", dest="add_source", action="store_false", help="Don't add _source_file column")
    parser.add_argument("--partition-by", nargs="*", default=None, help="Optional partition columns when writing parquet")
    args = parser.parse_args(argv)

    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")

    src = Path(args.dir)
    out = Path(args.out)

    if not src.exists():
        logger.error("Source directory %s does not exist", src)
        return 2

    combined = concat_excels(src, pattern=args.pattern, add_source=args.add_source)
    if combined.empty:
        logger.error("No data to write. Exiting.")
        return 3

    write_parquet(combined, out_path=out, partition_cols=args.partition_by)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
