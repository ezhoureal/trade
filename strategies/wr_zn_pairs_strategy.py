"""
WR-ZN Pairs Trading Strategy

This strategy implements a statistical arbitrage approach for trading pairs between
Wire Rod (WR) and Zinc (ZN) futures contracts using cointegration and z-score analysis.
"""

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
            print(f'SHORT at {self.data.datetime.date(0)} | z={zscore:.2f}')

        # Enter long spread (buy data1, sell data2)
        elif zscore < -self.params.entry_z and not self.in_long:
            self.close()
            self.buy(data=self.data1)
            self.sell(data=self.data2)
            self.in_long = True
            self.in_short = False
            print(f'LONG at {self.data.datetime.date(0)} | z={zscore:.2f}')

        # Exit positions when spread normalizes
        elif self.in_long and abs(zscore) < self.params.exit_z:
            self.close()
            self.in_long = False
            print(f'EXIT LONG at {self.data.datetime.date(0)} | z={zscore:.2f}')
        elif self.in_short and abs(zscore) < self.params.exit_z:
            self.close()
            self.in_short = False
            print(f'EXIT SHORT at {self.data.datetime.date(0)} | z={zscore:.2f}')

# --- Load Data ---
def load_parquet(path: str, contract: str):
    df = pd.read_parquet(path)
    df['Date'] = pd.to_datetime(df['Date'], format='%Y%m%d')
    df.set_index('Date', inplace=True)
    df = df[df['Contract'] == contract]
    print(df)
    return bt.feeds.PandasData(dataname=df, timeframe=bt.TimeFrame.Days)

cerebro = bt.Cerebro()

data1 = load_parquet('data/2024.parquet', 'zn2504')
data2 = load_parquet('data/2024.parquet', 'wr2501')
print(data1)

cerebro.adddata(data1, name='zn2504')
cerebro.adddata(data2, name='wr2501')

cerebro.addstrategy(PairTradingStrategy)
# cerebro.addanalyzer(bt.analyzers.TradeAnalyzer, _name='trades')
# cerebro.addanalyzer(bt.analyzers.SharpeRatio, _name='sharpe', riskfreerate=0.0)
# cerebro.addanalyzer(bt.analyzers.Returns, _name='returns')

# Get analyzer results
result = cerebro.run()

# strat = result[0]

# # --- Logging ---
# total_trades = strat.analyzers.trades.get_analysis().total.closed if hasattr(strat.analyzers.trades.get_analysis().total, 'closed') else strat.analyzers.trades.get_analysis().total
# print(f'Total closed trades: {total_trades}')

# # Total return
# returns = strat.analyzers.returns.get_analysis()
# total_return = returns.get('rtot', None)
# if total_return is None:  # fallback
#     total_return = strat.broker.getvalue() - cerebro.broker.startingcash
# print(f'Total return: {total_return:.2f}')

# # Sharpe ratio
# sharpe = strat.analyzers.sharpe.get_analysis().get('sharperatio', None)
# print(f'Sharpe ratio: {sharpe:.2f}')
