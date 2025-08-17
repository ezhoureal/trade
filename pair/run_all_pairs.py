"""
Run pairs trading strategy on all pairs from pairs_coint.csv

This script reads cointegrated pairs from the CSV file and runs the pairs trading
strategy on each pair using concatenated parquet data from all years.
"""

import argparse
import csv
import glob
import os
import sys
from typing import List, Tuple

import pandas as pd

# Import the backtest strategy
from backtest import run_strategy


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
                  results_file: str = None):
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
    
    # Results storage
    results = []
    
    print(f"\nRunning strategy on {len(pairs)} pairs...")
    print(f"Strategy parameters: lookback={lookback}, entry_z={entry_z}, exit_z={exit_z}")
    print("=" * 80)
    
    for i, (contract1, contract2, pvalue) in enumerate(pairs, 1):
        print(f"\n[{i}/{len(pairs)}] Processing pair: {contract1} vs {contract2} (p-value: {pvalue:.2e})")
        
        try:
            result_data = run_strategy(
                contract1=contract1,
                contract2=contract2,
                data=data,
                lookback=lookback,
                entry_z=entry_z,
                exit_z=exit_z
            )
            # Store successful results
            result = {
                'contract1': contract1,
                'contract2': contract2,
                'pvalue': pvalue,
                **result_data
            }
            results.append(result)
            
            print(f"✅ Success: Return={result_data['total_return']:.2f}, "
                    f"Sharpe={result_data['sharpe_ratio']:.2f}, "
                    f"Trades={result_data['total_trades']}")
                
        except Exception as e:
            print(f"❌ Error: {e}")
            continue
    
    # Save results if requested
    if results_file:
        save_results(results, results_file)
    
    # Print summary
    print_summary(results)


def save_results(results: List[dict], filename: str):
    """Save results to CSV file."""
    if not results:
        print("No results to save")
        return
    
    fieldnames = ['contract1', 'contract2', 'pvalue', 'total_return', 'sharpe_ratio', 
                  'total_trades', 'winning_trades', 'losing_trades', 'win_rate', 'error']
    
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
    
    successful = [r for r in results if r.get('total_return') is not None]
    failed = [r for r in results if r.get('total_return') is None]
    
    print("\n" + "=" * 80)
    print("SUMMARY")
    print("=" * 80)
    print(f"Total pairs processed: {len(results)}")
    print(f"Successful runs: {len(successful)}")
    print(f"Failed runs: {len(failed)}")
    
    if successful:
        returns = [r['total_return'] for r in successful]
        sharpes = [r['sharpe_ratio'] for r in successful if r['sharpe_ratio'] is not None]
        
        print(f"\nReturn Statistics:")
        # print(f"  Mean return: {sum(returns) / len(returns):.2f}")
        print(f"  Best return: {max(returns):.2f}")
        print(f"  Worst return: {min(returns):.2f}")
        print(f"  Positive returns: {len([r for r in returns if r > 0])}/{len(returns)}")
        
        if sharpes:
            print(f"\nSharpe Ratio Statistics:")
            print(f"  Mean Sharpe: {sum(sharpes) / len(sharpes):.2f}")
            print(f"  Best Sharpe: {max(sharpes):.2f}")
        
        # Top 10 best performing pairs
        best_pairs = sorted(successful, key=lambda x: x['total_return'], reverse=True)[:10]
        print(f"\nTop 10 Best Performing Pairs:")
        for i, pair in enumerate(best_pairs, 1):
            print(f"  {i:2d}. {pair['contract1']}-{pair['contract2']}: "
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
  python run_all_pairs.py --max-pairs 50 --results results.csv
  python run_all_pairs.py --data-dir data --lookback 30 --entry-z 2.5
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
    parser.add_argument('--results', '-r', type=str,
                       help='Output CSV file for results (optional)')
    
    args = parser.parse_args()
    run_all_pairs(
        data_dir=args.data_dir,
        pairs_csv=args.pairs_csv,
        max_pairs=args.max_pairs,
        lookback=args.lookback,
        entry_z=args.entry_z,
        exit_z=args.exit_z,
        results_file=args.results
    )

if __name__ == "__main__":
    main()
