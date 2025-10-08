"""
intraday_bot.py
Simple intraday listener that computes RSI, MACD, KDJ, Volume signals and places orders via alpaca-py.

NOT FINANCIAL ADVICE. Test heavily in paper mode.
"""

import os
import time
from datetime import datetime, timezone, timedelta
from typing import Union, cast

import pandas as pd
from dotenv import load_dotenv

# Alpaca SDK
from alpaca.data.historical import StockHistoricalDataClient
from alpaca.data.live import StockDataStream
from alpaca.data.models import BarSet, Bar
from alpaca.data.requests import StockBarsRequest
from alpaca.data.timeframe import TimeFrame, TimeFrameUnit
from alpaca.trading.client import TradingClient
from alpaca.trading.requests import MarketOrderRequest
from alpaca.trading.enums import OrderSide, TimeInForce
from alpaca.trading.models import Position, Order
from alpaca.common.types import RawData

# load env
load_dotenv()
API_KEY = os.getenv("APCA_API_KEY_ID")
API_SECRET = os.getenv("APCA_API_SECRET_KEY")
API_BASE = os.getenv("APCA_API_BASE_URL", "https://paper-api.alpaca.markets/v2")
TRADING_MODE = os.getenv("TRADING_MODE", "paper")  # 'paper' or 'live'
SYMBOL = os.getenv("SYMBOL", "TSLA")
MAX_POSITION_RISK = float(os.getenv("MAX_POSITION_RISK", "0.5"))  # percent of equity per trade

if not API_KEY or not API_SECRET:
    raise RuntimeError("Set APCA_API_KEY_ID and APCA_API_SECRET_KEY in .env")

# clients
trading_client = TradingClient(API_KEY, API_SECRET, paper=(TRADING_MODE == "paper"))
hist_client = StockHistoricalDataClient(API_KEY, API_SECRET)

# data storage: keep last N 1-minute bars (we'll request historic initial chunk)
MAX_BARS = 200
bars_cols = ["t", "o", "h", "l", "c", "v"]
bars: pd.DataFrame = pd.DataFrame(columns=bars_cols).set_index("t")
MINUTE_UNIT = cast(TimeFrameUnit, TimeFrameUnit.Minute)
TIMEFRAME_MINUTE = TimeFrame(amount=1, unit=MINUTE_UNIT)

# utility indicator functions implemented in pandas-friendly way
def compute_rsi(series_close, period=14):
    delta = series_close.diff()
    gain = delta.clip(lower=0)
    loss = -1 * delta.clip(upper=0)
    # Wilder smoothing (EMA-like)
    avg_gain = gain.rolling(window=period, min_periods=period).mean()
    avg_loss = loss.rolling(window=period, min_periods=period).mean()
    # After initial, use Wilder smoothing
    # To keep it simple here, fallback to simple ratio when not enough history
    rs = avg_gain / (avg_loss + 1e-9)
    rsi = 100 - (100 / (1 + rs))
    return rsi

def compute_macd(series_close, fast=12, slow=26, signal=9):
    ema_fast = series_close.ewm(span=fast, adjust=False).mean()
    ema_slow = series_close.ewm(span=slow, adjust=False).mean()
    macd_line = ema_fast - ema_slow
    signal_line = macd_line.ewm(span=signal, adjust=False).mean()
    hist = macd_line - signal_line
    return macd_line, signal_line, hist

def compute_kdj(high, low, close, n=9, k_period=3, d_period=3):
    # KDJ is similar to Stochastic %K/%D with a J component
    low_min = low.rolling(window=n, min_periods=n).min()
    high_max = high.rolling(window=n, min_periods=n).max()
    rsv = (close - low_min) / (high_max - low_min + 1e-9) * 100
    K = rsv.ewm(alpha=1/k_period, adjust=False).mean()
    D = K.ewm(alpha=1/d_period, adjust=False).mean()
    J = 3 * K - 2 * D
    return K, D, J

# position sizing: simple fixed fraction of equity
def compute_order_qty(symbol_price, account, risk_frac=MAX_POSITION_RISK):
    equity = float(account.cash) + 0.0  # simple, in paper this is cash; for better use account.equity
    # use risk_frac fraction of equity to buy
    max_alloc = equity * risk_frac
    if symbol_price <= 0:
        return 0
    qty = int(max_alloc // symbol_price)
    return max(0, qty)


def ensure_order(result: Union[Order, RawData]) -> Order:
    if isinstance(result, Order):
        return result
    raise RuntimeError(f"Order submission failed, returned raw payload: {result}")

# initial historical fetch (1min bars) to seed indicators
# initial historical fetch (1min bars) to seed indicators
def _extract_bars(resp, symbol):
    if not isinstance(resp, BarSet):
        return []
    data = resp.data or {}
    return data.get(symbol) or data.get(symbol.upper()) or []


def seed_historical(symbol, limit=150):
    global bars
    now = datetime.now(timezone.utc)
    start = now - timedelta(minutes=limit * 2)
    request = StockBarsRequest(
        symbol_or_symbols=symbol,
        timeframe=TIMEFRAME_MINUTE,
        start=start,
        limit=limit,
    )
    resp = hist_client.get_stock_bars(request)
    bar_list = _extract_bars(resp, symbol)
    rows = []
    for b in bar_list:
        rows.append(
            {
                "t": b.timestamp.isoformat(),
                "o": b.open,
                "h": b.high,
                "l": b.low,
                "c": b.close,
                "v": b.volume,
            }
        )
    if not rows:
        return
    df = pd.DataFrame(rows).set_index("t")
    df.index = pd.to_datetime(df.index)
    df = df.sort_index()
    bars = df.tail(MAX_BARS).copy()
    print(f"[seed] loaded {len(bars)} bars")

# signal logic: combine RSI, MACD, KDJ, Volume heuristics
LAST_ORDER_TS = None
COOLDOWN_SECONDS = 30  # don't flood orders

def calc_macd_signal(macd_hist):
    if len(macd_hist) < 2:
        return 0
    macd_hist_last = macd_hist.iloc[-1]
    macd_hist_prev = macd_hist.iloc[-2]
    macd_cross_up = (macd_hist_prev < 0) and (macd_hist_last > 0)
    macd_cross_down = (macd_hist_prev > 0) and (macd_hist_last < 0)

    macd_bullish = macd_cross_up or (macd_hist_last > macd_hist_prev and macd_hist_prev < -0.2)
    macd_bearish = macd_cross_down or (macd_hist_last < macd_hist_prev and macd_hist_prev > 0.2)

    if macd_bullish:
        return 1
    elif macd_bearish:
        return -1
    else:
        return 0

def evaluate_and_trade():
    global bars, LAST_ORDER_TS
    if len(bars) < 30:
        return

    close = bars["c"]
    high = bars["h"]
    low = bars["l"]
    vol = bars["v"]

    # --- Compute indicators ---
    rsi = compute_rsi(close, period=14).iloc[-1]
    rsi_signal = 1 if rsi < 20 else (-1 if rsi > 80 else 0)

    macd_line, macd_signal, macd_hist = compute_macd(close)
    macd_signal_state = calc_macd_signal(macd_hist)

    K, D, J = compute_kdj(high, low, close)
    k_last = K.iloc[-1]
    d_last = D.iloc[-1]
    kdj_signal = 1 if k_last > d_last else (-1 if k_last < d_last else 0)

    vol_mean = vol.rolling(window=20, min_periods=10).mean().iloc[-1]
    vol_last = vol.iloc[-1]
    vol_spike = (vol_mean > 0) and (vol_last > vol_mean * 1.5)
    macd_last = macd_hist.iloc[-1]
    volume_signal = 1 if vol_spike and macd_last > 0 else (-1 if vol_spike and macd_last < 0 else 0)

    # --- Combine signals ---
    signals = [rsi_signal, macd_signal_state, kdj_signal, volume_signal]
    bullish_count = signals.count(1)
    bearish_count = signals.count(-1)
    buy_signal = bullish_count >= 3
    sell_signal = bearish_count >= 3

    # check cooldown
    now_ts = time.time()
    if LAST_ORDER_TS and (now_ts - LAST_ORDER_TS) < COOLDOWN_SECONDS:
        # in cooldown
        return

    account = trading_client.get_account()
    raw_positions = trading_client.get_all_positions()
    positions = {}
    for p in raw_positions:
        symbol = getattr(p, "symbol", None)
        if not symbol:
            continue
        positions[symbol] = p
    has_position = SYMBOL in positions

    print(f"[{bars.index[-1]}] rsi={rsi:.2f} macd_hist={macd_last:.6f} K={k_last:.2f} D={d_last:.2f} vol_spike={vol_spike}")

    try:
        if buy_signal and not has_position:
            # compute qty
            last_price = close.iloc[-1]
            qty = compute_order_qty(last_price, account)
            if qty <= 0:
                print("[trade] qty==0, skipping buy (insufficient allocation)")
                return
            print(f"[trade] BUY signal. Sending market buy order for {qty} {SYMBOL} at ~{last_price}")
            order_data = MarketOrderRequest(symbol=SYMBOL, qty=qty, limit_price=last_price, side=OrderSide.BUY, time_in_force=TimeInForce.DAY)
            order = ensure_order(trading_client.submit_order(order_data))
            LAST_ORDER_TS = time.time()
            print("[trade] buy order submitted:", order.id)
        elif sell_signal and has_position:
            pos = positions[SYMBOL]
            qty = abs(int(float(pos.qty)))
            if qty <= 0:
                return
            print(f"[trade] SELL signal. Closing position of {qty} {SYMBOL}")
            order_data = MarketOrderRequest(symbol=SYMBOL, qty=qty, limit_price=close.iloc[-1], side=OrderSide.SELL, time_in_force=TimeInForce.DAY)
            order = ensure_order(trading_client.submit_order(order_data))
            LAST_ORDER_TS = time.time()
            print("[trade] sell order submitted:", order.id)
    except Exception as e:
        print("[error] trading error:", e)

# streaming handler: on incoming 1-minute bars we update and evaluate
def on_trade_update(bar: Bar):
    # bar has attributes: symbol, t, o, h, l, c, v depending on stream event
    global bars
    try:
        t = pd.to_datetime(bar.timestamp)
    except Exception:
        t = pd.Timestamp.utcnow()
    new_row = {"o": bar.open, "h": bar.high, "l": bar.low, "c": bar.close, "v": bar.volume}
    # append or replace last bar by timestamp
    bars.loc[t] = new_row # type: ignore
    bars = bars[~bars.index.duplicated(keep='last')]
    bars = bars.sort_index().tail(MAX_BARS)
    # Evaluate and maybe trade
    evaluate_and_trade()

def run_stream():
    # seed historical bars so indicators have initial data
    seed_historical(SYMBOL, limit=150)

    # Create a StockDataStream (WebSocket) -- alpaca-py
    if not API_KEY or not API_SECRET:
        raise RuntimeError("Set APCA_API_KEY_ID and APCA_API_SECRET_KEY in .env")
    stream = StockDataStream(API_KEY, API_SECRET)
    try:
        async def handle_bar(bar):
            if isinstance(bar, Bar):
                on_trade_update(bar)
            else:
                print("[stream] received non-Bar data:", bar)

        stream.subscribe_bars(handle_bar, SYMBOL)

        print("[stream] starting stream for", SYMBOL)
        stream.run()  # blocking (runs forever)
    except Exception as e:
        print("[stream] stream error or api mismatch:", e)
        print("Falling back to polling loop.")
        # fallback: polling 1-min bars
        polling_loop()

def polling_loop():
    # fallback simple loop polls latest minute bar every 30s
    global bars
    while True:
        try:
            now = datetime.now(timezone.utc)
            start = now - timedelta(minutes=20)
            request = StockBarsRequest(
                symbol_or_symbols=SYMBOL,
                timeframe=TIMEFRAME_MINUTE,
                start=start,
                end=now,
                limit=100,
            )
            resp = hist_client.get_stock_bars(request)
            bar_list = _extract_bars(resp, SYMBOL)
            rows = []
            for b in bar_list:
                rows.append(
                    {
                        "t": b.timestamp.isoformat(),
                        "o": b.open,
                        "h": b.high,
                        "l": b.low,
                        "c": b.close,
                        "v": b.volume,
                    }
                )
            if rows:
                df = pd.DataFrame(rows).set_index("t")
                df.index = pd.to_datetime(df.index)
                df = df.sort_index()
                bars = df.tail(MAX_BARS).copy()
                evaluate_and_trade()
            time.sleep(30)
        except Exception as e:
            print("[polling] error:", e)
            time.sleep(10)

if __name__ == "__main__":
    print("Starting intraday bot for", SYMBOL, "in", TRADING_MODE, "mode")
    try:
        run_stream()
    except KeyboardInterrupt:
        print("Stopping bot cleanly.")
