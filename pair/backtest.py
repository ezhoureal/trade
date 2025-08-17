"""
Pairs Trading Strategy

This strategy implements a statistical arbitrage approach for trading pairs between
futures contracts using cointegration and z-score analysis.
"""

import argparse
import sys
import backtrader as bt
import pandas as pd
import numpy as np

# --- Strategy ---
class PairTradingStrategy(bt.Strategy):
    params = (
        ('lookback', 20),     # rolling window for mean/std
        ('entry_z', 2.0),     # entry threshold
        ('exit_z', 0.5),      # exit threshold
    )

    def __init__(self):
        # Assume two data feeds: datas[0] = ZN, datas[1] = WR
        self.data1 = self.datas[0]
        self.data2 = self.datas[1]

        # Store past spread for rolling stats
        self.spread_history = []

        # Track position state
        self.in_long = False
        self.in_short = False

    def next(self):
        # Current spread
        spread = self.data1.close[0] - self.data2.close[0]
        self.spread_history.append(spread)

        # Wait until we have enough data
        if len(self.spread_history) < self.params.lookback:
            return

        # Compute rolling mean and std
        window = self.spread_history[-self.params.lookback:]
        mean = np.mean(window)
        std = np.std(window)
        zscore = (spread - mean) / std

        # --- Entry / Exit Rules ---
        # Enter short spread (sell data1, buy data2)
        if zscore > self.params.entry_z and not self.in_short:
            self.close()  # close any opposite positions
            self.sell(data=self.data1)
            self.buy(data=self.data2)
            self.in_short = True
            self.in_long = False
            if hasattr(self, 'debug') and self.debug:
                print(f'SHORT at {self.data.datetime.date(0)} | z={zscore:.2f}')

        # Enter long spread (buy data1, sell data2)
        elif zscore < -self.params.entry_z and not self.in_long:
            self.close()
            self.buy(data=self.data1)
            self.sell(data=self.data2)
            self.in_long = True
            self.in_short = False
            if hasattr(self, 'debug') and self.debug:
                print(f'LONG at {self.data.datetime.date(0)} | z={zscore:.2f}')

        # Exit positions when spread normalizes
        elif self.in_long and abs(zscore) < self.params.exit_z:
            self.close()
            self.in_long = False
            if hasattr(self, 'debug') and self.debug:
                print(f'EXIT LONG at {self.data.datetime.date(0)} | z={zscore:.2f}')
        elif self.in_short and abs(zscore) < self.params.exit_z:
            self.close()
            self.in_short = False
            if hasattr(self, 'debug') and self.debug:
                print(f'EXIT SHORT at {self.data.datetime.date(0)} | z={zscore:.2f}')


def get_contract_data(df: pd.DataFrame, symbol: str):
    """Extract and prepare data for a specific contract symbol."""
    data = df[df['Contract'] == symbol]
    if data.empty:
        raise ValueError(f"No data found for contract '{symbol}'")
    
    data = data.copy()
    data['Date'] = pd.to_datetime(data['Date'], format='%Y%m%d')
    data.set_index('Date', inplace=True)
    return bt.feeds.PandasData(dataname=data, timeframe=bt.TimeFrame.Days)


def run_strategy(contract1: str, contract2: str, data: pd.DataFrame, 
                lookback: int = 20, entry_z: float = 2.0, exit_z: float = 0.5) -> dict:
    """Run the pairs trading strategy with specified contracts."""
    cerebro = bt.Cerebro()
    # Add data feeds
    try:
        data1 = get_contract_data(data, contract1)
        data2 = get_contract_data(data, contract2)
        cerebro.adddata(data1, name=contract1)
        cerebro.adddata(data2, name=contract2)
    except ValueError as e:
        print(f"Error loading contract data: {e}")
        return None
    
    # Add strategy with parameters
    cerebro.addstrategy(PairTradingStrategy, 
                       lookback=lookback, 
                       entry_z=entry_z, 
                       exit_z=exit_z)
    
    # Add analyzers
    cerebro.addanalyzer(bt.analyzers.TradeAnalyzer, _name='trades')
    cerebro.addanalyzer(bt.analyzers.SharpeRatio, _name='sharpe', riskfreerate=0.0)
    cerebro.addanalyzer(bt.analyzers.Returns, _name='returns')
    # Run strategy
    result = cerebro.run()[0]
    
    # Extract results
    returns = result.analyzers.returns.get_analysis()
    sharpe = result.analyzers.sharpe.get_analysis()
    trades = result.analyzers.trades.get_analysis()
    if sharpe['sharperatio'] is None:
        raise ValueError("Sharpe ratio wasn't generated, not enough samples.")
    return {
        'total_return': returns.get('rtot', 0.0),
        'sharpe_ratio': sharpe.get('sharperatio', 0.0),
        'total_trades': trades.total.total,
        'winning_trades': trades.won.total,
        'losing_trades': trades.lost.total,
        'win_rate': trades.won.total / trades.total.total if trades.total.total > 0 else 0.0,
    }

def main():
    """Main function to parse arguments and run the strategy."""
    parser = argparse.ArgumentParser(
        description="Run pairs trading strategy on two futures contracts",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python strategies/pair.py zn2404 wr2401 --data data/2023.parquet
  python strategies/pair.py rb2405 hc2405 --data data/2024.parquet --lookback 30 --entry-z 2.5
        """
    )
    
    parser.add_argument('contract1', help='First contract symbol (e.g., zn2404)')
    parser.add_argument('contract2', help='Second contract symbol (e.g., wr2401)')
    parser.add_argument('data', help='Path to the parquet data file')
    parser.add_argument('--lookback', '-l', type=int, default=20,
                       help='Rolling window size for mean/std calculation (default: 20)')
    parser.add_argument('--entry-z', '-e', type=float, default=2.0,
                       help='Z-score threshold for entry (default: 2.0)')
    parser.add_argument('--exit-z', '-x', type=float, default=0.5,
                       help='Z-score threshold for exit (default: 0.5)')
    
    args = parser.parse_args()
    try:
        df = pd.read_parquet(args.data)
        res = run_strategy(
            contract1=args.contract1,
            contract2=args.contract2,
            data=df,
            lookback=args.lookback,
            entry_z=args.entry_z,
            exit_z=args.exit_z
        )

        # Display results
        print("-" * 60)
        print("RESULTS:")
        
        # Total return
        returns = res.analyzers.returns.get_analysis()
        total_return = returns.get('rtot', None)
        if total_return is None:  # fallback
            total_return = res.broker.getvalue() - cerebro.broker.startingcash
        print(f'Total return: {total_return:.2f}')
        
        # Sharpe ratio
        sharpe = res.analyzers.sharpe.get_analysis().get('sharperatio', None)
        if sharpe:
            print(f'Sharpe ratio: {sharpe:.2f}')
        
        # Trade analysis
        trades = res.analyzers.trades.get_analysis()
        if trades.total.total > 0:
            print(f"Total trades: {trades.total.total}")
            print(f"Winning trades: {trades.won.total}")
            print(f"Losing trades: {trades.lost.total}")
            if trades.won.total > 0:
                print(f"Average winning trade: {trades.won.pnl.average:.2f}")
            if trades.lost.total > 0:
                print(f"Average losing trade: {trades.lost.pnl.average:.2f}")
    except Exception as e:
        print(f"Error running strategy: {e}")
        sys.exit(1)


if __name__ == "__main__":
    main()
