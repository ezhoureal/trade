"""
Run pairs trading strategy on all pairs from pairs_coint.csv

This script reads cointegrated pairs from the CSV file and runs the pairs trading
strategy on each pair using concatenated parquet data from all years.
"""

import argparse
import csv
import glob
import os
from typing import List, Tuple
from concurrent.futures import ProcessPoolExecutor, as_completed
from multiprocessing import cpu_count

import pandas as pd

# Import the backtest strategy
from strategies.pair_trade import run_strategy


def load_concatenated_data(data_dir: str) -> pd.DataFrame:
    """Load and concatenate all parquet files from the data directory."""
    print(f"Loading parquet files from {data_dir}...")
    
    parquet_files = glob.glob(os.path.join(data_dir, "*.parquet"))
    if not parquet_files:
        raise FileNotFoundError(f"No parquet files found in {data_dir}")
    
    dfs = []
    for file in sorted(parquet_files):
        print(f"  Reading {file}")
        dfs.append(pd.read_parquet(file))
    
    df = pd.concat(dfs, ignore_index=True)
    print(f"Concatenated data shape: {df.shape}")
    return df


def load_pairs_from_csv(csv_file: str, max_pairs: int = None) -> List[Tuple[str, str, float]]:
    """Load contract pairs from the cointegration results CSV file."""
    pairs = []
    
    with open(csv_file, 'r') as f:
        reader = csv.DictReader(f)
        for i, row in enumerate(reader):
            if max_pairs and i >= max_pairs:
                break
            
            contract_a = row['A']
            contract_b = row['B']
            pvalue = float(row['pvalue'])
            
            pairs.append((contract_a, contract_b, pvalue))
    
    print(f"Loaded {len(pairs)} pairs from {csv_file}")
    return pairs

def run_all_pairs(data_dir: str, pairs_csv: str, max_pairs: int = None,
                  lookback: int = 20, entry_z: float = 2.0, exit_z: float = 0.5,
                  results_file: str = None, n_workers: int = None):
    """Run pairs trading strategy on all pairs from the CSV file."""
    
    # Load concatenated data
    try:
        data = load_concatenated_data(data_dir)
    except Exception as e:
        print(f"Error loading data: {e}")
        return
    
    # Load pairs
    try:
        pairs = load_pairs_from_csv(pairs_csv, max_pairs)
    except Exception as e:
        print(f"Error loading pairs: {e}")
        return
    
    if not pairs:
        print("No pairs to process")
        return
    
    # Set number of workers
    if n_workers is None:
        n_workers = min(cpu_count(), len(pairs))
    
    # Results storage
    results = []
    
    print(f"\nRunning strategy on {len(pairs)} pairs using {n_workers} workers...")
    print(f"Strategy parameters: lookback={lookback}, entry_z={entry_z}, exit_z={exit_z}")
    print("=" * 80)
    
    # Prepare arguments for worker processes
    worker_args = [
        (contract1, contract2, data, lookback, entry_z, exit_z)
        for contract1, contract2, pvalue in pairs
    ]
    
    # Process pairs in parallel
    completed_count = 0
    with ProcessPoolExecutor(max_workers=n_workers) as executor:
        # Submit all jobs
        future_to_pair = {
            executor.submit(run_strategy, *args): (args[0], args[1])
            for args in worker_args
        }
        
        # Process completed futures
        for future in as_completed(future_to_pair):
            contract1, contract2 = future_to_pair[future]
            completed_count += 1

            print(f"\n[{completed_count}/{len(pairs)}] Processing pair: {contract1} vs {contract2}")

            try:
                result = future.result()
                result.update({"contract_pair": f'{contract1} - {contract2}'})
                results.append(result)
                print(f"✅ Success: Return={result['total_return']:.2f}, "
                        f"Sharpe={result['sharpe_ratio']:.2f}, "
                        f"Trades={result['total_trades']}")
            except Exception as e:
                print(f"❌ Error processing pair: {contract1} vs {contract2}")
                print(f"   {e}")

    # Sort results by Sharpe ratio in descending order
    results.sort(key=lambda x: x['sharpe_ratio'], reverse=True)
    if results_file:
        save_results(results, results_file)
    
    # Print summary
    print_summary(results)


def save_results(results: List[dict], filename: str):
    """Save results to CSV file."""
    if not results:
        print("No results to save")
        return
    
    fieldnames = ['contract_pair', 'total_return', 'sharpe_ratio', 
                  'total_trades', 'winning_trades', 'losing_trades', 'win_rate']
    
    with open(filename, 'w', newline='') as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(results)
    
    print(f"\nResults saved to {filename}")


def print_summary(results: List[dict]):
    """Print summary of results."""
    if not results:
        print("\nNo results to summarize")
        return
    
    print("\n" + "=" * 80)
    print("SUMMARY")
    print("=" * 80)
    print(f"Total pairs processed: {len(results)}")

    returns = [r['total_return'] for r in results]
    sharpes = [r['sharpe_ratio'] for r in results]

    print(f"\nReturn Statistics:")
    print(f"  Positive returns: {len([r for r in returns if r > 0])}/{len(returns)}")
    
    print(f"\nSharpe Ratio Statistics:")
    if sharpes:
        print(f"  Best Sharpe: {max(sharpes):.2f}")
    else:
        print(f"  No results with positive Sharpe ratios")
    
    # Top 10 best performing pairs
    best_pairs = results[:10]
    print(f"\nTop 10 Best Performing Pairs:")
    for i, pair in enumerate(best_pairs, 1):
        print(f"  {i:2d}. {pair['contract_pair']}: "
                f"Return={pair['total_return']:.2f}, "
                f"Sharpe={pair['sharpe_ratio']:.2f}, "
                f"Trades={pair['total_trades']}")


def main():
    """Main function to parse arguments and run all pairs."""
    parser = argparse.ArgumentParser(
        description="Run pairs trading strategy on all pairs from cointegration results",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python run_all_pairs.py
  python run_all_pairs.py --max-pairs 50 -o results.csv
  python run_all_pairs.py --data-dir data --lookback 30 --entry-z 2.5
  python run_all_pairs.py --workers 4 --max-pairs 100
        """
    )
    
    parser.add_argument('--data-dir', '-d', default='data',
                       help='Directory containing parquet files (default: data)')
    parser.add_argument('--pairs-csv', '-p', default='data/pairs_coint.csv',
                       help='CSV file with cointegrated pairs (default: data/pairs_coint.csv)')
    parser.add_argument('--max-pairs', '-n', type=int,
                       help='Maximum number of pairs to process (default: all)')
    parser.add_argument('--lookback', '-l', type=int, default=20,
                       help='Rolling window size for mean/std calculation (default: 20)')
    parser.add_argument('--entry-z', '-e', type=float, default=2.0,
                       help='Z-score threshold for entry (default: 2.0)')
    parser.add_argument('--exit-z', '-x', type=float, default=0.5,
                       help='Z-score threshold for exit (default: 0.5)')
    parser.add_argument('--out', '-o', type=str,
                       help='Output CSV file for results (optional)')
    parser.add_argument('--workers', '-w', type=int,
                       help='Number of worker processes (default: auto-detect based on CPU cores)')
    
    args = parser.parse_args()
    run_all_pairs(
        data_dir=args.data_dir,
        pairs_csv=args.pairs_csv,
        max_pairs=args.max_pairs,
        lookback=args.lookback,
        entry_z=args.entry_z,
        exit_z=args.exit_z,
        results_file=args.out,
        n_workers=args.workers
    )

if __name__ == "__main__":
    main()
