import backtrader as bt
import backtrader.indicators as btind
import backtrader.feeds as btfeeds
import csv
from datetime import datetime

class RsiStrategy(bt.Strategy):
    def __init__(self):
        self.rsi = btind.RelativeStrengthIndex()

    def next(self):
        if not self.position:  # No position
            if self.rsi < 30:
                self.buy()
            elif self.rsi > 70:
                self.sell()
        else:  # Have position
            if self.position.size > 0:  # Long position
                if self.rsi > 70:
                    self.close()
            else:  # Short position
                if self.rsi < 30:
                    self.close()

class MovingAverageBreakoutStrategy(bt.Strategy):
    params = dict(
        ma_period=50,
        stop_loss=0.02,   # 2% stop loss
        take_profit=0.04  # 4% take profit
    )

    def __init__(self):
        self.sma = bt.indicators.MovingAverageSimple(self.data.close, period=self.p.ma_period)

    def next(self):
        if not self.position:  # No position
            if self.data.close[0] > self.sma[0]:
                self.buy()
            elif self.data.close[0] < self.sma[0]:
                self.sell()
        else:  # Manage open positions
            if self.position.size > 0:  # Long position
                if (self.data.close[0] < self.sma[0] or
                    self.data.close[0] < self.position.price * (1 - self.p.stop_loss) or
                    self.data.close[0] > self.position.price * (1 + self.p.take_profit)):
                    self.close()
            elif self.position.size < 0:  # Short position
                if (self.data.close[0] > self.sma[0] or
                    self.data.close[0] > self.position.price * (1 + self.p.stop_loss) or
                    self.data.close[0] < self.position.price * (1 - self.p.take_profit)):
                    self.close()


def main():
    cerebro = bt.Cerebro()
    
    clean_file = 'clean_gold_future.csv'
    # Create a GenericCSVData feed from the clean CSV
    data = btfeeds.GenericCSVData(
        dataname=clean_file,
        dtformat='%m/%d/%Y',
        timeframe=bt.TimeFrame.Days,
        openinterest=-1,
        datetime=0,
        open=1,
        high=2,
        low=3,
        close=4,
        volume=-1,
        skiprows=1  # Skip header row
    )
    cerebro.adddata(data)
    cerebro.addstrategy(RsiStrategy)
    result = cerebro.run()
    cerebro.plot(volume=False)

if __name__ == "__main__":
    main()
