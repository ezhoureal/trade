"""
Dynamic Copper-Fuel Oil Pairs Trading Strategy
"""

import pandas as pd
import numpy as np
import argparse
import sys
import os
from collections import defaultdict, deque
from typing import Dict, Tuple
import backtrader as bt

# Add parent directory to path
sys.path.append(os.path.join(os.path.dirname(__file__), '..'))


class DynamicPairSelector:
    """Manages dynamic pair selection and performance tracking during execution."""
    
    def __init__(self, lookback_performance: int, min_trades: int):
        self.lookback_performance = lookback_performance
        self.min_trades = min_trades
        # Use regular dict structure instead of defaultdict with complex initialization
        self.pair_performance = {}
        
    def _get_pair_stats(self, pair: Tuple[str, str]) -> dict:
        """Get or create pair statistics."""
        if pair not in self.pair_performance:
            self.pair_performance[pair] = {
                'returns': deque(maxlen=self.lookback_performance),
                'trades': 0,
                'wins': 0,
                'total_return': 0.0,
                'sharpe': 0.0,
                'last_updated': 0,
                'avg_return_per_trade': 0.0,
                'success_score': 0.0
            }
        return self.pair_performance[pair]
        
    def update_pair_performance(self, pair: Tuple[str, str], trade_return: float, bar_count: int):
        """Update performance metrics for a pair after a trade."""
        stats = self._get_pair_stats(pair)
        stats['returns'].append(trade_return)
        stats['trades'] += 1
        stats['total_return'] += trade_return
        if trade_return > 0:
            stats['wins'] += 1
        stats['last_updated'] = bar_count
        
        # Calculate metrics
        if stats['trades'] > 0:
            stats['avg_return_per_trade'] = stats['total_return'] / stats['trades']
            win_rate = stats['wins'] / stats['trades']
            
            # Calculate Sharpe ratio if we have enough data
            if len(stats['returns']) >= 5:
                returns_array = np.array(stats['returns'])
                if np.std(returns_array) > 0:
                    stats['sharpe'] = float(np.mean(returns_array) / np.std(returns_array))
                else:
                    stats['sharpe'] = 0.0
            
            # Calculate success score (combines return, win rate, and consistency)
            stats['success_score'] = float(
                stats['avg_return_per_trade'] * 0.4 +
                win_rate * 0.3 +
                stats['sharpe'] * 0.3
            )

class DynamicCopperFuelStrategy(bt.Strategy):
    """Dynamic pairs trading strategy for copper and fuel oil contracts."""
    
    params = (
        ('lookback_zscore', 20),        # rolling window for z-score calculation
        ('lookback_performance', 50),    # window for performance evaluation
        ('entry_z', 2.0),               # entry threshold
        ('exit_z', 0.5),                # exit threshold
        ('pair_evaluation_freq', 10),    # how often to evaluate new pairs
        ('max_active_pairs', 3),        # maximum number of active pairs
        ('min_volume_threshold', 50),    # minimum daily volume requirement
        ('exploration_rate', 0.2),      # rate of exploring new pairs
    )

    def __init__(self):
        self.bar_count = 0
        self.pair_selector = DynamicPairSelector(
            lookback_performance=self.p.lookback_performance,
            min_trades=3
        )
        
        # Access the full dataset from the master data feed
        self.full_data = self.datas[0]._full_dataset
        self.current_date = None
        
        # Get unique contracts
        all_contracts = self.full_data['Contract'].unique()
        self.copper_contracts = [c for c in all_contracts if c.startswith('cu')]
        self.fuel_contracts = [c for c in all_contracts if c.startswith('fu')]
        
        print(f"Initialized with {len(self.copper_contracts)} copper and {len(self.fuel_contracts)} fuel contracts")
        
        # Track active positions and spread histories
        self.active_positions = {}  # {pair: {'type': 'long'/'short', 'entry_z': float, 'entry_bar': int}}
        self.spread_histories = defaultdict(lambda: deque(maxlen=self.p.lookback_zscore))
        
        # Pair exploration tracking
        self.pair_exploration_queue = []
        self.last_pair_evaluation = 0
        
        # Performance tracking
        self.trade_log = []
        
    def next(self):
        self.bar_count += 1
        
        # Get current date from the master data feed
        try:
            self.current_date = self.data.datetime.date(0)
        except:
            print("Error getting current date")
            return
            
        if self.bar_count <= 25 or self.bar_count % 50 == 0:
            print(f"Bar {self.bar_count}, Date: {self.current_date}")
        
        current_ts = pd.Timestamp(self.datas[0].datetime.date(0))
        today_data = self.full_data[self.full_data['Date'] == current_ts]
        if today_data.empty:
            return

        self._update_spread_histories(today_data)
        if self.bar_count % 100 == 0:
            self._cleanup_expired_contracts()

        if self.bar_count < self.p.lookback_zscore:
            # Skip initial bars to build history
            return
            
        self._manage_existing_positions()

        # Extract contract lists from filtered data
        filtered_data = self._filter_top_contracts_by_oi(today_data)
        if not filtered_data.empty:
            top_cu_contracts = [c for c in filtered_data['Contract'].unique() if c.startswith('cu')]
            top_fu_contracts = [c for c in filtered_data['Contract'].unique() if c.startswith('fu')]
            
            # Try to enter pairs only from top contracts
            for cu_contract in top_cu_contracts:
                for fu_contract in top_fu_contracts:
                    pair = (cu_contract, fu_contract)
                    # Only try pairs that have sufficient history and aren't already active
                    if (len(self.active_positions) < self.p.max_active_pairs and 
                        pair not in self.active_positions and 
                        len(self.spread_histories[pair]) >= self.p.lookback_zscore):
                        self._try_enter_pair(pair)

    def _filter_top_contracts_by_oi(self, today_data: pd.DataFrame) -> pd.DataFrame:
        """Filter to keep only top 3 contracts by Open Interest for copper and fuel oil."""
        filtered_contracts = []
        
        cu_data = today_data[today_data['Contract'].str.startswith('cu')]
        if not cu_data.empty:
            # Sort by OI descending and take top 3
            cu_top = cu_data.nlargest(3, 'OI')
            filtered_contracts.append(cu_top)
        
        fu_data = today_data[today_data['Contract'].str.startswith('fu')]
        if not fu_data.empty:
            # Sort by OI descending and take top 3
            fu_top = fu_data.nlargest(3, 'OI')
            filtered_contracts.append(fu_top)
        
        # Combine filtered data
        if filtered_contracts:
            filtered_data = pd.concat(filtered_contracts, ignore_index=True)
            return filtered_data
        else:
            return pd.DataFrame()  # Return empty DataFrame if no data
    
    def _update_spread_histories(self, today_data: pd.DataFrame):
        """Update spread histories for pairs with data today (optimized)."""
        cu_prices = {}
        fu_prices = {}

        for _, row in today_data.iterrows():
            contract = row['Contract']
            price = float(row['Close'])
            
            if contract.startswith('cu'):
                cu_prices[contract] = price
            elif contract.startswith('fu'):
                fu_prices[contract] = price
        
        # Only process pairs where both contracts have data today
        pairs_updated = 0
        for cu_contract, cu_price in cu_prices.items():
            for fu_contract, fu_price in fu_prices.items():
                pair = (cu_contract, fu_contract)
                spread = cu_price - fu_price
                self.spread_histories[pair].append(spread)
                pairs_updated += 1
        
        # Periodic summary (less frequent than before)
        if self.bar_count % 50 == 1:  # Every 50 bars instead of every bar
            print(f"  Bar {self.bar_count}: Updated {pairs_updated} pairs, "
                  f"{len(cu_prices)} copper, {len(fu_prices)} fuel contracts")
            
    def _cleanup_expired_contracts(self):
        """Remove pairs containing expired contracts from spread_histories."""
        if self.current_date is None:
            return
            
        current_date = pd.Timestamp(self.current_date)
        pairs_to_remove = []
        
        for pair in list(self.spread_histories.keys()):
            cu_contract, fu_contract = pair
            
            # Extract expiration dates from contract names
            cu_expired = self._is_contract_expired(cu_contract, current_date)
            fu_expired = self._is_contract_expired(fu_contract, current_date)
            
            # Remove pair if either contract is expired
            if cu_expired or fu_expired:
                pairs_to_remove.append(pair)
        
        # Remove expired pairs
        for pair in pairs_to_remove:
            del self.spread_histories[pair]
            # Also remove from active positions if present
            if pair in self.active_positions:
                del self.active_positions[pair]
        
        if pairs_to_remove and self.bar_count % 100 == 1:  # Log cleanup occasionally
            print(f"  Cleaned up {len(pairs_to_remove)} expired pairs from spread histories")
        elif self.bar_count % 100 == 1:  # Log even when no cleanup needed
            print(f"  Checked for expired contracts, found {len(pairs_to_remove)} to remove")
    
    def _is_contract_expired(self, contract: str, current_date: pd.Timestamp) -> bool:
        """Check if a contract is expired based on its name."""
        try:
            # Extract year and month from contract name
            # Contract format appears to be like 'cu2408', 'fu2501' etc.
            if len(contract) >= 6:
                # Extract last 4 digits as YYMM
                year_month = contract[-4:]
                year = int('20' + year_month[:2])  # Convert YY to 20YY
                month = int(year_month[2:])
                
                # Create expiration date (assume 15th of the month for simplicity)
                expiration_date = pd.Timestamp(year=year, month=month, day=15)
                
                return current_date > expiration_date
        except (ValueError, IndexError):
            # If we can't parse the date, assume it's not expired
            pass
        
        return False
            
    def _try_enter_pair(self, pair: Tuple[str, str]):
        """Try to enter a position in the given pair."""
        cu_name, fu_name = pair
        
        # Calculate z-score
        spread_history = list(self.spread_histories[pair])
        current_spread = spread_history[-1]
        mean_spread = np.mean(spread_history)
        std_spread = np.std(spread_history)
        
        if std_spread == 0:
            return
        
        z_score = (current_spread - mean_spread) / std_spread
        
        # Calculate position size based on z-score strength (risk management)
        position_size = min(10, int(abs(z_score) * 2))  # Much smaller position size
        if position_size < 1:
            position_size = 1
        
        # Check entry conditions
        if z_score > self.p.entry_z:
            # Enter short spread (sell copper, buy fuel) - simulate with short position
            # Place actual trade with Backtrader
            self.sell(size=position_size)
            
            self.active_positions[pair] = {
                'type': 'short_spread',
                'entry_z': z_score,
                'entry_bar': self.bar_count,
                'entry_spread': current_spread,
                'position_size': position_size,
                'bt_position': 'short'
            }
            # print(f"    ENTER SHORT SPREAD {cu_name}/{fu_name} at bar {self.bar_count}, z={z_score:.2f}, size={position_size}")
                
        elif z_score < -self.p.entry_z:
            # Enter long spread (buy copper, sell fuel) - simulate with long position
            # Place actual trade with Backtrader
            self.buy(size=position_size)
            
            self.active_positions[pair] = {
                'type': 'long_spread',
                'entry_z': z_score,
                'entry_bar': self.bar_count,
                'entry_spread': current_spread,
                'position_size': position_size,
                'bt_position': 'long'
            }
            # print(f"    ENTER LONG SPREAD {cu_name}/{fu_name} at bar {self.bar_count}, z={z_score:.2f}, size={position_size}")
    
    def _manage_existing_positions(self):
        """Manage all existing positions."""
        positions_to_close = []
        
        for pair, position_info in self.active_positions.items():
            if len(self.spread_histories[pair]) < self.p.lookback_zscore:
                print('warning, script logic error. insufficient data for pair:', pair)
                continue
            spread_history = list(self.spread_histories[pair])
            current_spread = spread_history[-1]
            mean_spread = np.mean(spread_history)
            std_spread = np.std(spread_history)
            
            if std_spread <= 0:
                continue
            z_score = (current_spread - mean_spread) / std_spread
            
            # Check exit conditions
            bars_held = self.bar_count - position_info['entry_bar']
            
            # Exit if z-score reverted, position held too long, or stop-loss triggered
            if (abs(z_score) < self.p.exit_z or 
                # bars_held > 50 or  # Force close after 50 bars
                abs(z_score) > 5.0):  # Stop-loss if z-score gets too extreme
                reason = "reversion" if abs(z_score) < self.p.exit_z else ("timeout" if bars_held > 50 else "stop-loss")
                self._close_position(pair, z_score, position_info, reason)
                positions_to_close.append(pair)
        
        # Remove closed positions
        for pair in positions_to_close:
            if pair in self.active_positions:
                del self.active_positions[pair]
    
    def _close_position(self, pair: Tuple[str, str], z_score: float, position_info: dict, reason: str = "reversion"):
        """Close a position and update performance metrics."""
        try:
            # Close the actual Backtrader position
            if position_info.get('bt_position') == 'long':
                order = self.sell(size=position_info.get('position_size', 1))
            elif position_info.get('bt_position') == 'short':
                order = self.buy(size=position_info.get('position_size', 1))  # Cover short
    
            # Calculate trade return (simplified)
            entry_z = position_info['entry_z']
            trade_return = z_score - entry_z
            
            if position_info['type'] == 'short_spread':
                trade_return = -trade_return  # Invert for short spread
            
            # Update pair performance
            self.pair_selector.update_pair_performance(pair, trade_return, self.bar_count)
            
            # Log trade
            self.trade_log.append({
                'pair': f"{pair[0]}/{pair[1]}",
                'type': position_info['type'],
                'entry_bar': position_info['entry_bar'],
                'exit_bar': self.bar_count,
                'entry_z': entry_z,
                'exit_z': z_score,
                'return': trade_return,
                'reason': reason
            })
            
            cu_name, fu_name = pair
            # print(f"    EXIT {position_info['type'].upper()} {cu_name}/{fu_name} at bar {self.bar_count}, "
            #       f"entry_z={entry_z:.2f}, exit_z={z_score:.2f}, return={trade_return:.2f}, reason={reason}")
            
        except Exception as e:
            print(f"Error closing position for {pair}: {e}")

def load_all_contracts(data_path: str) -> Tuple[Dict[str, bt.feeds.PandasData], pd.DataFrame]:
    """Load all copper and fuel oil contracts and create a single master data feed."""
    import glob
    import os
    
    # If data_path is a file, use its directory; if it's a directory, use it directly
    if os.path.isfile(data_path):
        data_dir = os.path.dirname(data_path)
        parquet_files = [data_path]
    else:
        data_dir = data_path
        # Find all parquet files in the data directory
        parquet_files = glob.glob(os.path.join(data_dir, "*.parquet"))
    
    if not parquet_files:
        raise ValueError(f"No parquet files found in {data_dir}")
    
    print(f"Loading data from {len(parquet_files)} parquet files: {[os.path.basename(f) for f in parquet_files]}")
    
    # Load and combine all parquet files
    dfs = []
    for file in sorted(parquet_files):
        df_temp = pd.read_parquet(file)
        df_temp['source_file'] = os.path.basename(file)
        dfs.append(df_temp)
    
    df = pd.concat(dfs, ignore_index=True)
    df['Date'] = pd.to_datetime(df['Date'], format='%Y%m%d')
    
    # Remove duplicates (in case there are overlapping dates between files)
    df = df.drop_duplicates(subset=['Contract', 'Date'], keep='last')
    df = df.sort_values(['Contract', 'Date'])
    
    # Get all copper and fuel oil contracts
    all_contracts = df['Contract'].unique()
    copper_contracts = [c for c in all_contracts if c.startswith('cu')]
    fuel_contracts = [c for c in all_contracts if c.startswith('fu')]
    
    print(f"Combined data: {len(df)} rows, {len(all_contracts)} unique contracts")
    print(f"Found {len(copper_contracts)} copper contracts and {len(fuel_contracts)} fuel contracts")
    print(f"Date range: {df['Date'].min()} to {df['Date'].max()}")
    
    # Create a master dataset by selecting the most active contract per day
    # This ensures we have continuous data for backtrader
    master_data = []
    
    # Get all unique dates
    all_dates = sorted(df['Date'].unique())
    
    for date in all_dates:
        day_data = df[df['Date'] == date]
        
        # Find the most active copper contract for this day (highest volume)
        cu_day = day_data[day_data['Contract'].str.startswith('cu')]
        if not cu_day.empty:
            # Use the contract with highest volume, or first one if volume not available
            if 'Volume' in cu_day.columns:
                best_cu = cu_day.loc[cu_day['Volume'].idxmax()]
            else:
                best_cu = cu_day.iloc[0]
            
            master_data.append({
                'Date': date,
                'open': best_cu['Open'],
                'high': best_cu['High'],
                'low': best_cu['Low'],
                'close': best_cu['Close'],
                'volume': best_cu.get('Volume', 1000),
                'primary_contract': best_cu['Contract']
            })
    
    master_df = pd.DataFrame(master_data)
    master_df.set_index('Date', inplace=True)
    
    # Create a single data feed for backtrader
    data_feeds = {
        'master': bt.feeds.PandasData(dataname=master_df)
    }
    
    # Store the full dataset in the data feed for strategy access
    data_feeds['master']._full_dataset = df
    
    print(f"Created master data feed with {len(master_df)} trading days")
    
    return data_feeds, df


def run_dynamic_strategy(data_path: str, **kwargs) -> dict:
    """Run the dynamic copper-fuel oil pairs trading strategy."""
    cerebro = bt.Cerebro()
    
    # Load all contract data from parquet files
    data_feeds, original_df = load_all_contracts(data_path)
    
    if not data_feeds:
        raise ValueError("No valid contract data found")
    
    # Add all data feeds to cerebro
    for name, feed in data_feeds.items():
        cerebro.adddata(feed, name=name)
    
    # Add strategy with parameters
    cerebro.addstrategy(
        DynamicCopperFuelStrategy,
        lookback_zscore=kwargs.get('lookback_zscore', 20),
        entry_z=kwargs.get('entry_z', 2.0),
        exit_z=kwargs.get('exit_z', 0.5),
        pair_evaluation_freq=kwargs.get('pair_evaluation_freq', 10),
        max_active_pairs=kwargs.get('max_active_pairs', 3)
    )
    
    # Add analyzers
    cerebro.addanalyzer(bt.analyzers.TradeAnalyzer, _name='trades')
    cerebro.addanalyzer(bt.analyzers.SharpeRatio, _name='sharpe', riskfreerate=0.0)
    cerebro.addanalyzer(bt.analyzers.Returns, _name='returns')
    cerebro.addanalyzer(bt.analyzers.DrawDown, _name='drawdown')
    
    # Set initial cash
    cerebro.broker.setcash(100000.0)
    
    print(f"Starting backtest with {len(data_feeds)} contracts...")
    
    # Run strategy
    results = cerebro.run()
    result = results[0]
    
    # Extract results
    returns = result.analyzers.returns.get_analysis()
    sharpe = result.analyzers.sharpe.get_analysis()
    trades = result.analyzers.trades.get_analysis()
    drawdown = result.analyzers.drawdown.get_analysis()
    
    # Extract metrics with safe defaults
    sharpe_value = sharpe.get('sharperatio') if sharpe else None
    sharpe_ratio = sharpe_value if sharpe_value is not None else 0.0
    
    # Safe extraction of trade statistics
    try:
        total_trades = trades.total.total
    except (KeyError, AttributeError):
        total_trades = 0
        
    try:
        winning_trades = trades.won.total
    except (KeyError, AttributeError):
        winning_trades = 0
        
    try:
        losing_trades = trades.lost.total
    except (KeyError, AttributeError):
        losing_trades = 0
    
    win_rate = winning_trades / total_trades if total_trades > 0 else 0.0
    
    # Get pair performance from strategy
    pair_performance = {}
    trade_log = []
    if hasattr(result, 'pair_selector') and hasattr(result, 'trade_log'):
        for pair, stats in result.pair_selector.pair_performance.items():
            pair_performance[f"{pair[0]}/{pair[1]}"] = {
                'trades': stats['trades'],
                'wins': stats['wins'],
                'total_return': stats['total_return'],
                'sharpe': stats['sharpe'],
                'success_score': stats['success_score']
            }
        trade_log = result.trade_log

    return {
        'total_return': returns.get('rtot', 0.0),
        'sharpe_ratio': sharpe_ratio,
        'total_trades': total_trades,
        'winning_trades': winning_trades,
        'losing_trades': losing_trades,
        'win_rate': win_rate,
        'max_drawdown': drawdown.get('max', {}).get('drawdown', 0.0) if drawdown else 0.0,
        'pair_performance': pair_performance,
        'trade_log': trade_log,
        'final_value': cerebro.broker.getvalue()
    }


def main():
    """Main function to run the dynamic pairs trading strategy."""
    parser = argparse.ArgumentParser(
        description="Run dynamic copper-fuel oil pairs trading strategy",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python strategies/dynamic_cu_fu_pairs.py data/
  python strategies/dynamic_cu_fu_pairs.py data/2024.parquet
  python strategies/dynamic_cu_fu_pairs.py data/ --lookback-zscore 30 --entry-z 2.5
        """
    )
    
    parser.add_argument('data', help='Path to the parquet data file or directory containing parquet files')
    parser.add_argument('--lookback-zscore', '-l', type=int, default=20,
                       help='Rolling window size for z-score calculation (default: 20)')
    parser.add_argument('--entry-z', '-e', type=float, default=2.0,
                       help='Z-score threshold for entry (default: 2.0)')
    parser.add_argument('--exit-z', '-x', type=float, default=0.5,
                       help='Z-score threshold for exit (default: 0.5)')
    parser.add_argument('--max-pairs', '-p', type=int, default=3,
                       help='Maximum number of active pairs (default: 3)')
    parser.add_argument('--eval-freq', '-f', type=int, default=10,
                       help='Pair evaluation frequency in bars (default: 10)')
    
    args = parser.parse_args()
    
    try:
        results = run_dynamic_strategy(
            data_path=args.data,
            lookback_zscore=args.lookback_zscore,
            entry_z=args.entry_z,
            exit_z=args.exit_z,
            max_active_pairs=args.max_pairs,
            pair_evaluation_freq=args.eval_freq
        )
        
        # Display results
        print("\n" + "="*80)
        print("DYNAMIC COPPER-FUEL OIL PAIRS TRADING RESULTS")
        print("="*80)
        
        print(f"Final Portfolio Value: ${results['final_value']:,.2f}")
        print(f"Total Return: {results['total_return']:.2%}")
        print(f"Sharpe Ratio: {results['sharpe_ratio']:.2f}")
        print(f"Maximum Drawdown: {results['max_drawdown']:.2%}")
        print(f"Total Trades: {results['total_trades']}")
        print(f"Winning Trades: {results['winning_trades']}")
        print(f"Losing Trades: {results['losing_trades']}")
        print(f"Win Rate: {results['win_rate']:.2%}")
        
        if results['pair_performance']:
            print("\nTop Performing Pairs:")
            print("-" * 50)
            sorted_pairs = sorted(
                results['pair_performance'].items(),
                key=lambda x: x[1]['success_score'],
                reverse=True
            )
            for pair_name, stats in sorted_pairs[:10]:
                if stats['trades'] > 0:
                    print(f"{pair_name}: {stats['trades']} trades, "
                          f"Win Rate: {stats['wins']/max(stats['trades'],1):.1%}, "
                          f"Score: {stats['success_score']:.2f}")
                      
    except Exception as e:
        print(f"Error running strategy: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    main()
