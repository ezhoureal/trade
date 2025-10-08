"""Backtest engine for the intraday Alpaca strategy."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from types import SimpleNamespace
from typing import Dict, List, Optional, cast

import pandas as pd

from alpaca.data.models import Bar
from alpaca.data.requests import StockBarsRequest
from alpaca.data.enums import DataFeed
from alpaca.trading.enums import OrderSide

import sys
import os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import intra_day as strategy


@dataclass
class BacktestTrade:
    timestamp: datetime
    symbol: str
    side: OrderSide
    qty: int
    price: float


@dataclass
class BacktestResult:
    trades: List[BacktestTrade]
    realized_pnl: float
    ending_equity: float
    starting_cash: float

    @property
    def total_return(self) -> float:
        if self.starting_cash == 0:
            return 0.0
        return (self.ending_equity - self.starting_cash) / self.starting_cash

    @property
    def total_trades(self) -> int:
        return len(self.trades)


@dataclass
class _Position:
    symbol: str
    qty: float = 0.0
    avg_entry_price: float = 0.0

    def snapshot(self) -> SimpleNamespace:
        return SimpleNamespace(
            symbol=self.symbol,
            qty=str(self.qty),
            avg_entry_price=str(self.avg_entry_price),
        )


class BacktestTradingClient:
    """Minimal trading client that mimics the subset used by the live strategy."""

    def __init__(self, starting_cash: float) -> None:
        self.starting_cash = starting_cash
        self.cash: float = starting_cash
        self.positions: Dict[str, _Position] = {}
        self.order_seq = 0
        self.current_price: float = 0.0
        self.current_time: datetime = datetime.now(timezone.utc)
        self.current_symbol: str = strategy.SYMBOL
        self.realized_pnl: float = 0.0
        self.trades: List[BacktestTrade] = []

    # --- helpers ---------------------------------------------------------
    def update_market(self, timestamp: datetime, symbol: str, price: float) -> None:
        self.current_time = timestamp
        self.current_symbol = symbol
        self.current_price = price

    # --- strategy interface ----------------------------------------------
    def get_account(self) -> SimpleNamespace:
        return SimpleNamespace(cash=str(self.cash))

    def get_all_positions(self) -> List[SimpleNamespace]:
        return [pos.snapshot() for pos in self.positions.values() if pos.qty > 0]

    def submit_order(self, order_data) -> SimpleNamespace:
        symbol = getattr(order_data, "symbol", self.current_symbol)
        qty = int(getattr(order_data, "qty", 0))
        side = getattr(order_data, "side", OrderSide.BUY)
        price = self.current_price

        if qty <= 0:
            raise RuntimeError("Backtest submit_order received zero quantity")
        if price <= 0:
            raise RuntimeError("Backtest price not initialised before order")

        position = self.positions.setdefault(symbol, _Position(symbol=symbol))

        if side == OrderSide.BUY:
            cost = price * qty
            self.cash -= cost
            new_qty = position.qty + qty
            position.avg_entry_price = (
                (position.avg_entry_price * position.qty + cost) / new_qty
                if new_qty > 0
                else 0.0
            )
            position.qty = new_qty
        else:
            # SELL path
            if position.qty < qty:
                raise RuntimeError(
                    f"Attempting to sell {qty} shares while only holding {position.qty}"
                )
            proceeds = price * qty
            self.cash += proceeds
            realized = (price - position.avg_entry_price) * qty
            self.realized_pnl += realized
            position.qty -= qty
            if position.qty <= 0:
                position.qty = 0.0
                position.avg_entry_price = 0.0

        trade = BacktestTrade(
            timestamp=self.current_time,
            symbol=symbol,
            side=side,
            qty=qty,
            price=price,
        )
        self.trades.append(trade)

        order_id = f"bt-{self.order_seq}"
        self.order_seq += 1
        return SimpleNamespace(id=order_id, symbol=symbol, qty=str(qty), side=side, filled_avg_price=str(price))


class BacktestEngine:
    """Orchestrates a historical replay by reusing the live strategy module."""

    def __init__(
        self,
        symbol: Optional[str] = None,
        start: Optional[datetime] = None,
        end: Optional[datetime] = None,
        starting_cash: float = 100_000.0,
    ) -> None:
        self.symbol = symbol or strategy.SYMBOL
        self.start = start or (datetime.now(timezone.utc) - timedelta(days=5))
        self.end = end or datetime.now(timezone.utc)
        self.starting_cash = starting_cash
        self.client = BacktestTradingClient(starting_cash)

    # ------------------------------------------------------------------
    def run(self) -> BacktestResult:
        bars = self._load_bars()
        if not bars:
            raise RuntimeError("No historical bars returned for backtest")

        strategy.trading_client = self.client
        strategy.ensure_order = lambda order: order
        strategy.COOLDOWN_SECONDS = 0
        strategy.LAST_ORDER_TS = None
        strategy.bars = pd.DataFrame(columns=strategy.bars_cols).set_index("t")

        for bar in bars:
            timestamp = self._ensure_utc(bar.timestamp)
            self.client.update_market(timestamp, self.symbol, bar.close)
            fake_bar = SimpleNamespace(
                timestamp=timestamp,
                open=bar.open,
                high=bar.high,
                low=bar.low,
                close=bar.close,
                volume=bar.volume,
            )
            strategy.on_trade_update(cast(Bar, fake_bar))

        last_price = bars[-1].close
        open_positions_value = sum(pos.qty * last_price for pos in self.client.positions.values())
        ending_equity = self.client.cash + open_positions_value

        return BacktestResult(
            trades=self.client.trades,
            realized_pnl=self.client.realized_pnl,
            ending_equity=ending_equity,
            starting_cash=self.starting_cash,
        )

    # ------------------------------------------------------------------
    def _load_bars(self) -> List[Bar]:
        hist_client = strategy.hist_client
        timeframe = strategy.TIMEFRAME_MINUTE
        bars: List[Bar] = []
        cursor = self.start

        while cursor < self.end:
            window_end = min(cursor + timedelta(days=7), self.end)
            request = StockBarsRequest(
                symbol_or_symbols=self.symbol,
                timeframe=timeframe,
                start=cursor,
                feed=DataFeed.IEX,
                end=window_end,
                limit=10_000,
            )
            response = hist_client.get_stock_bars(request)
            chunk = strategy._extract_bars(response, self.symbol)
            if not chunk:
                break
            bars.extend(chunk)
            last_ts = chunk[-1].timestamp
            cursor = self._ensure_utc(last_ts) + timedelta(minutes=1)

        bars.sort(key=lambda b: b.timestamp)
        return bars

    @staticmethod
    def _ensure_utc(ts) -> datetime:
        if isinstance(ts, datetime):
            if ts.tzinfo is None:
                return ts.replace(tzinfo=timezone.utc)
            return ts.astimezone(timezone.utc)
        return pd.to_datetime(ts).tz_localize(timezone.utc)


def run_backtest(
    symbol: Optional[str] = None,
    start: Optional[str] = None,
    end: Optional[str] = None,
    starting_cash: float = 100_000.0,
) -> BacktestResult:
    """Convenience function to run a backtest from CLI notebooks."""

    start_dt = datetime.fromisoformat(start) if start else None
    end_dt = datetime.fromisoformat(end) if end else None

    engine = BacktestEngine(symbol=symbol, start=start_dt, end=end_dt, starting_cash=starting_cash)
    result = engine.run()

    print(
        "Backtest complete",
        f"Trades: {result.total_trades}",
        f"Realized PnL: {result.realized_pnl:.2f}",
        f"Ending Equity: {result.ending_equity:.2f}",
        f"Return: {result.total_return*100:.2f}%",
        sep=" | ",
    )

    return result


if __name__ == "__main__":
    run_backtest('TSLA')
