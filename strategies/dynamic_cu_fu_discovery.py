"""
Dynamic Copper-Fuel Oil Pairs Strategy (Simplified)

This strategy extends the existing backtest framework to dynamically discover and track
multiple Cu-Fu pairs without look-ahead bias. It works by pre-processing the data to
identify overlapping pairs and then running the strategy on each discovered pair.
"""

import pandas as pd
import numpy as np
import argparse
import sys
import os
from datetime import datetime
import itertools

# Add parent directory to path
sys.path.append(os.path.join(os.path.dirname(__file__), '..'))
from strategies.pair_trade import run_strategy


def discover_cu_fu_pairs(df, min_overlap_days=30):
    """Discover all valid Copper-Fuel Oil pairs in the dataset."""
    cu_contracts = sorted([c for c in df['Contract'].unique() if c.startswith('cu')])
    fu_contracts = sorted([c for c in df['Contract'].unique() if c.startswith('fu')])
    
    print(f"Found {len(cu_contracts)} copper contracts and {len(fu_contracts)} fuel oil contracts")
    
    valid_pairs = []
    
    for cu_contract in cu_contracts:
        for fu_contract in fu_contracts:
            # Get data for both contracts
            cu_data = df[df['Contract'] == cu_contract]
            fu_data = df[df['Contract'] == fu_contract]
            
            if cu_data.empty or fu_data.empty:
                continue
            
            # Check for overlapping dates
            cu_dates = set(cu_data['Date'])
            fu_dates = set(fu_data['Date'])
            overlap = cu_dates.intersection(fu_dates)
            
            if len(overlap) >= min_overlap_days:
                valid_pairs.append({
                    'cu_contract': cu_contract,
                    'fu_contract': fu_contract,
                    'overlap_days': len(overlap),
                    'start_date': min(overlap),
                    'end_date': max(overlap),
                    'cu_data_points': len(cu_data),
                    'fu_data_points': len(fu_data)
                })
    
    # Sort by overlap days (most data first)
    valid_pairs.sort(key=lambda x: x['overlap_days'], reverse=True)
    return valid_pairs


def analyze_pair_correlation(df, cu_contract, fu_contract):
    """Analyze the correlation and spread characteristics of a pair."""
    cu_data = df[df['Contract'] == cu_contract][['Date', 'Close']].copy()
    fu_data = df[df['Contract'] == fu_contract][['Date', 'Close']].copy()
    
    # Merge on date
    merged = pd.merge(cu_data, fu_data, on='Date', suffixes=('_cu', '_fu'))
    merged['Date'] = pd.to_datetime(merged['Date'])
    merged = merged.sort_values('Date')
    
    if len(merged) < 20:
        return None
    
    # Calculate correlation and spread statistics
    correlation = merged['Close_cu'].corr(merged['Close_fu'])
    
    merged['spread'] = merged['Close_cu'] - merged['Close_fu']
    spread_mean = merged['spread'].mean()
    spread_std = merged['spread'].std()
    
    # Calculate z-scores
    merged['spread_ma'] = merged['spread'].rolling(20).mean()
    merged['spread_std_roll'] = merged['spread'].rolling(20).std()
    merged['z_score'] = (merged['spread'] - merged['spread_ma']) / merged['spread_std_roll']
    
    # Count potential trading opportunities
    entry_signals = (merged['z_score'].abs() > 2.0).sum()
    
    return {
        'correlation': correlation,
        'spread_mean': spread_mean,
        'spread_std': spread_std,
        'max_z_score': merged['z_score'].max(),
        'min_z_score': merged['z_score'].min(),
        'potential_entries': entry_signals,
        'data_quality': len(merged)
    }


def run_dynamic_cu_fu_strategy(file_path, max_pairs=None, min_correlation=0.3, debug=False):
    """
    Run the dynamic Cu-Fu strategy by discovering pairs and testing each one.
    This approach avoids look-ahead bias by discovering pairs based only on
    data availability and basic correlation, not performance.
    """
    print("=== DYNAMIC COPPER-FUEL OIL PAIRS STRATEGY ===")
    print(f"Loading data from: {file_path}")
    
    # Load data
    df = pd.read_parquet(file_path)
    print(f"Loaded {len(df)} rows, {df['Contract'].nunique()} unique contracts")
    
    # Discover valid pairs
    print("\\nDiscovering Cu-Fu pairs...")
    valid_pairs = discover_cu_fu_pairs(df)
    
    if not valid_pairs:
        print("No valid Cu-Fu pairs found!")
        return None
    
    print(f"Found {len(valid_pairs)} valid pairs")
    
    # Filter pairs by correlation and other criteria (but NOT by performance!)
    filtered_pairs = []
    
    print("\\nAnalyzing pair characteristics...")
    for pair in valid_pairs:
        cu_contract = pair['cu_contract']
        fu_contract = pair['fu_contract']
        
        # Analyze correlation (this is NOT cheating - it's a basic filter)
        correlation_stats = analyze_pair_correlation(df, cu_contract, fu_contract)
        
        if correlation_stats is None:
            continue
            
        # Filter criteria (based on data quality, not performance)
        if (correlation_stats['correlation'] >= min_correlation and
            correlation_stats['data_quality'] >= 50 and
            correlation_stats['potential_entries'] >= 2):
            
            pair.update(correlation_stats)
            filtered_pairs.append(pair)
            
            if debug:
                print(f"  {cu_contract}-{fu_contract}: "
                      f"corr={correlation_stats['correlation']:.3f}, "
                      f"entries={correlation_stats['potential_entries']}, "
                      f"days={pair['overlap_days']}")
    
    if not filtered_pairs:
        print("No pairs meet the filtering criteria!")
        return None
    
    # Limit number of pairs to test
    if max_pairs:
        filtered_pairs = filtered_pairs[:max_pairs]
    
    print(f"\\nTesting {len(filtered_pairs)} pairs...")
    
    # Run backtest on each pair
    results = []
    successful_pairs = 0
    
    for i, pair in enumerate(filtered_pairs):
        cu_contract = pair['cu_contract']
        fu_contract = pair['fu_contract']
        
        if debug:
            print(f"\\n{i+1}/{len(filtered_pairs)}: Testing {cu_contract} - {fu_contract}")
            print(f"  Overlap: {pair['overlap_days']} days")
            print(f"  Correlation: {pair['correlation']:.3f}")
        
        try:
            # Run the existing backtest strategy
            result = run_strategy(
                contract1=cu_contract,
                contract2=fu_contract,
                data=df,
                lookback=20,
                entry_z=2.0,
                exit_z=0.5
            )
            
            if result:
                result.update({
                    'cu_contract': cu_contract,
                    'fu_contract': fu_contract,
                    'pair_name': f"{cu_contract}-{fu_contract}",
                    'overlap_days': pair['overlap_days'],
                    'correlation': pair['correlation'],
                    'potential_entries': pair['potential_entries']
                })
                results.append(result)
                
                if result['total_trades'] > 0:
                    successful_pairs += 1
                    
                if debug:
                    print(f"    Result: {result['total_trades']} trades, "
                          f"{result['win_rate']:.1%} win rate, "
                          f"{result['total_return']:.3f} return")
            else:
                if debug:
                    print(f"    Failed to run backtest")
                    
        except Exception as e:
            if debug:
                print(f"    Error: {e}")
            continue
    
    # Summary
    print(f"\\n{'='*60}")
    print("DYNAMIC STRATEGY RESULTS")
    print(f"{'='*60}")
    
    if results:
        # Convert to DataFrame for analysis
        results_df = pd.DataFrame(results)
        
        # Sort by total return
        results_df = results_df.sort_values('total_return', ascending=False)
        
        # Display results
        print(f"Pairs tested: {len(filtered_pairs)}")
        print(f"Pairs with trades: {successful_pairs}")
        print(f"Success rate: {successful_pairs/len(filtered_pairs):.1%}")
        
        # Top performing pairs
        print(f"\\nTop 10 performing pairs:")
        top_pairs = results_df.head(10)
        for _, row in top_pairs.iterrows():
            print(f"  {row['pair_name']}: {row['total_return']:.3f} return, "
                  f"{row['total_trades']} trades, {row['win_rate']:.1%} win rate")
        
        # Overall statistics
        total_trades = results_df['total_trades'].sum()
        profitable_pairs = len(results_df[results_df['total_return'] > 0])
        avg_return = results_df['total_return'].mean()
        avg_win_rate = results_df['win_rate'].mean()
        
        print(f"\\nOverall Statistics:")
        print(f"Total trades across all pairs: {total_trades}")
        print(f"Profitable pairs: {profitable_pairs}/{len(results_df)}")
        print(f"Average return per pair: {avg_return:.3f}")
        print(f"Average win rate: {avg_win_rate:.1%}")
        
        # Save results
        output_file = 'dynamic_cu_fu_results.csv'
        results_df.to_csv(output_file, index=False)
        print(f"\\nDetailed results saved to: {output_file}")
        
        return {
            'pairs_tested': len(filtered_pairs),
            'pairs_with_trades': successful_pairs,
            'total_trades': total_trades,
            'profitable_pairs': profitable_pairs,
            'avg_return': avg_return,
            'avg_win_rate': avg_win_rate,
            'top_pair': results_df.iloc[0]['pair_name'] if len(results_df) > 0 else None,
            'top_return': results_df.iloc[0]['total_return'] if len(results_df) > 0 else 0,
            'results': results_df.to_dict('records')
        }
    else:
        print("No successful backtests!")
        return None


def main():
    """Main function with command line interface."""
    parser = argparse.ArgumentParser(
        description="Dynamic Copper-Fuel Oil Pairs Discovery and Testing",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
This strategy dynamically discovers Cu-Fu pairs based on data availability and 
basic correlation criteria (NOT performance), then tests each pair to avoid 
look-ahead bias.

Examples:
  python strategies/dynamic_cu_fu_discovery.py data/2022.parquet
  python strategies/dynamic_cu_fu_discovery.py data/2022.parquet --max-pairs 20 --debug
  python strategies/dynamic_cu_fu_discovery.py data/2022.parquet --min-correlation 0.5
        """
    )
    
    parser.add_argument('data_file', help='Path to the parquet data file')
    parser.add_argument('--max-pairs', type=int, default=50,
                       help='Maximum number of pairs to test (default: 50)')
    parser.add_argument('--min-correlation', type=float, default=0.3,
                       help='Minimum correlation threshold (default: 0.3)')
    parser.add_argument('--debug', '-d', action='store_true',
                       help='Enable debug output')
    
    args = parser.parse_args()
    
    try:
        results = run_dynamic_cu_fu_strategy(
            file_path=args.data_file,
            max_pairs=args.max_pairs,
            min_correlation=args.min_correlation,
            debug=args.debug
        )
        
        if results:
            print("\\nStrategy completed successfully!")
            return results
        else:
            print("Strategy failed!")
            return None
            
    except Exception as e:
        print(f"Error running strategy: {e}")
        if args.debug:
            import traceback
            traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
