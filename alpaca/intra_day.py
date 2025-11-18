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
import numpy as np

# Alpaca SDK
from alpaca.data.historical import StockHistoricalDataClient
from alpaca.data.live import StockDataStream
from alpaca.data.models import BarSet, Bar, Trade
from alpaca.data.requests import StockBarsRequest
from alpaca.data.timeframe import TimeFrame, TimeFrameUnit
from alpaca.trading.client import TradingClient
from alpaca.trading.requests import LimitOrderRequest, MarketOrderRequest
from alpaca.trading.enums import OrderSide, TimeInForce
from alpaca.trading.models import Order, Position
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

LAST_ORDER_TS = None
COOLDOWN_SECONDS = 30

# Market close time (3:55 PM ET)
MARKET_CLOSE_TIME = "15:55"

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

def macd_components(series_close, fast=12, slow=26, signal=9):
    ema_fast = series_close.ewm(span=fast, adjust=False).mean()
    ema_slow = series_close.ewm(span=slow, adjust=False).mean()
    macd_line = ema_fast - ema_slow  # DIF
    signal_line = macd_line.ewm(span=signal, adjust=False).mean()  # DEA
    hist = macd_line - signal_line
    return macd_line, signal_line, hist

def macd_signal_strict(macd_line, signal_line, hist, lookback=3):
    """
    Returns 1 for bullish, -1 for bearish, 0 neutral.
    Rules:
      - Bullish: DIF>DEA (last), both above 0, hist positive and |hist| increasing over last `lookback` bars
      - Bearish: DIF<DEA, both below 0, hist negative and |hist| increasing over last `lookback` bars
    """
    if len(hist) < lookback:
        return (0, 0)
    dif_last = macd_line.iloc[-1]
    dea_last = signal_line.iloc[-1]
    hist_vals = hist.iloc[-lookback:]
    # check monotonic increase in absolute hist magnitude
    abs_hist = np.abs(hist_vals.values)
    abs_increasing = np.all(np.diff(abs_hist) > 0)

    if dif_last > dea_last and dif_last > 0:
        weak_macd = 1
    elif dif_last < dea_last and dif_last < 0:
        weak_macd = -1
    else:
        weak_macd = 0

    if dif_last > dea_last and dif_last > 0 and dea_last > 0 and hist_vals.iloc[-1] > 0 and abs_increasing:
        return (1, weak_macd)
    if dif_last < dea_last and dif_last < 0 and dea_last < 0 and hist_vals.iloc[-1] < 0 and abs_increasing:
        return (-1, weak_macd)
    return (0, weak_macd)

def kdj_signal_strict(K, D, J, lookback=3):
    """
    Returns 1 if K上穿D 且 三线从超卖区域(<20) 向上反转；
            -1 if K下穿D 且 三线从超买区域(>80) 向下反转；
            0 otherwise.
    We check previous `lookback` bars to ensure they were in oversold/overbought region and now moving out.
    """
    if len(K) < lookback + 1:
        return (0, 0)
    k_last = K.iloc[-1]
    d_last = D.iloc[-1]
    k_prev = K.iloc[-2]
    d_prev = D.iloc[-2]

    # check recent history for being in oversold/overbought
    recent_K = K.iloc[-(lookback+1):-1]
    recent_D = D.iloc[-(lookback+1):-1]

    if k_last > d_last and recent_K.max() < 20:
        weak_kdj = 1
    elif k_last < d_last and recent_K.min() > 80:
        weak_kdj = -1
    else:
        weak_kdj = 0
    # "from 超卖区向上反转" -> recent all < 20 and now K>D and K rising
    if (recent_K.max() < 20) and (recent_D.max() < 20) and (k_last > d_last) and (k_last > k_prev):
        return (1, weak_kdj)
    # "从 超买区 向下反转"
    if (recent_K.min() > 80) and (recent_D.min() > 80) and (k_last < d_last) and (k_last < k_prev):
        return (-1, weak_kdj)
    return (0, weak_kdj)

def volume_signal_strict(vol_series, close_series):
    """
    Rules:
      - vol increase >=20% vs previous bar => potential spike.
      - If vol spike + price up from prev => bullish volume confirmation -> 1
      - If vol spike + price down from prev => bearish volume confirmation -> -1
      - else 0
    """
    if len(vol_series) < 2 or len(close_series) < 2:
        return (0, 0)
    v_last = vol_series.iloc[-1]
    v_prev = vol_series.iloc[-2]
    c_last = close_series.iloc[-1]
    c_prev = close_series.iloc[-2]
    
    weak_vol = 1 if v_last > v_prev else -1

    if v_prev <= 0:
        return (0, weak_vol)
    spike = (v_last >= v_prev * 1.2)  # >=20% 增量
    if not spike:
        return (0, weak_vol)
    # price direction confirmation
    if c_last > c_prev:
        return (1, weak_vol)
    elif c_last < c_prev:
        return (-1, weak_vol)
    else:
        return (0, weak_vol)

def rsi_signals(close_series):
    """
    Returns tuple: (rsi14, rsi6, rsi12, rsi_flag)
    rsi_flag: 1 if multi RSI conditions satisfied (RSI14 from <30 and now >50 AND RSI6>RSI12),
              -1 for opposite, 0 otherwise.
    We'll inspect history to detect 'from <30' or 'from >70' within recent bars.
    """
    rsi14_series = compute_rsi(close_series, period=14)
    rsi6_series = compute_rsi(close_series, period=6)
    rsi12_series = compute_rsi(close_series, period=12)
    if len(rsi14_series) < 5:
        return (rsi14_series.iloc[-1], rsi6_series.iloc[-1], rsi12_series.iloc[-1], 0)
    rsi14_last = rsi14_series.iloc[-1]
    rsi14_prev = rsi14_series.iloc[-5:-1]  # lookback to see where it came from
    rsi6_last = rsi6_series.iloc[-1]
    rsi12_last = rsi12_series.iloc[-1]

    # 多头：从 30 以下回升，并已穿越 50，且 RSI6 > RSI12
    came_from_oversold = (rsi14_prev.max() < 30) or (rsi14_prev.mean() < 30)
    bullish_cross_50 = (rsi14_last > 50) and came_from_oversold and (rsi6_last > rsi12_last)

    # 空头：从 70 以上向下，并已跌破 50，且 RSI6 < RSI12
    came_from_overbought = (rsi14_prev.min() > 70) or (rsi14_prev.mean() > 70)
    bearish_cross_50 = (rsi14_last < 50) and came_from_overbought and (rsi6_last < rsi12_last)

    if bullish_cross_50:
        return (rsi14_last, rsi6_last, rsi12_last, 1)
    if bearish_cross_50:
        return (rsi14_last, rsi6_last, rsi12_last, -1)
    return (rsi14_last, rsi6_last, rsi12_last, 0)

# ---------- 在 evaluate_and_trade 中替换信号计算与合成 ----------
def evaluate_and_trade():
    global bars, LAST_ORDER_TS
    if len(bars) < 40:
        return

    close = bars["c"]
    high = bars["h"]
    low = bars["l"]
    vol = bars["v"]

    # --- Compute indicators ---
    # MACD components
    macd_line, signal_line, macd_hist = macd_components(close)
    (macd_sig, weak_macd) = macd_signal_strict(macd_line, signal_line, macd_hist, lookback=3)

    # KDJ
    K, D, J = compute_kdj(high, low, close, n=9, k_period=3, d_period=3)
    (kdj_sig, weak_kdj) = kdj_signal_strict(K, D, J, lookback=3)

    # Volume
    (vol_sig, weak_vol) = volume_signal_strict(vol, close)

    # RSI family
    rsi14, rsi6, rsi12, rsi_sig = rsi_signals(close)

    # resonance signal of the 4 metrics
    if rsi14 > 50 and weak_vol == 1 and weak_macd == 1 and weak_kdj == 1:
        resonance_sig = 1
    elif rsi14 < 50 and weak_vol == -1 and weak_macd == -1 and weak_kdj == -1:
        resonance_sig = -1
    else:
        resonance_sig = 0

    signals = [macd_sig, kdj_sig, vol_sig, rsi_sig, resonance_sig]
    is_multi = signals.count(1) >= 3
    is_short = signals.count(-1) >= 3

    # Debug logging (only when a candidate signal appears)
    if is_multi or is_short:
        print(f"[{bars.index[-1]}] SIGNAL: multi={is_multi} short={is_short} "
              f"macd={macd_sig} kdj={kdj_sig} vol={vol_sig} rsi={rsi_sig} "
              f"rsi14={rsi14:.2f} rsi6={rsi6:.2f} rsi12={rsi12:.2f} macd_hist={macd_hist.iloc[-1]:.6f}")

    # Respect cooldown
    now_ts = time.time()
    if LAST_ORDER_TS and (now_ts - LAST_ORDER_TS) < COOLDOWN_SECONDS:
        # print("[cooldown] skipping due to recent order")
        return

    account = trading_client.get_account()
    raw_positions = trading_client.get_all_positions()
    positions = {getattr(p, "symbol", ""): p for p in raw_positions if getattr(p, "symbol", None)}
    has_position = SYMBOL in positions

    try:
        if is_multi:
            last_price = close.iloc[-1]
            qty = compute_order_qty(last_price, account)
            if qty <= 0:
                return
            print(f"[trade] BUY signal. Sending limit buy order for {qty} {SYMBOL} at ~{last_price}")
            order_data = LimitOrderRequest(symbol=SYMBOL, qty=qty, limit_price=last_price, side=OrderSide.BUY, time_in_force=TimeInForce.DAY)
            ensure_order(trading_client.submit_order(order_data))
            LAST_ORDER_TS = time.time()
        elif is_short and has_position:
            pos = positions[SYMBOL]
            qty = abs(int(float(pos.qty)))
            if qty <= 0:
                return
            last_price = close.iloc[-1]
            print(f"[trade] SELL (4-way). Closing {qty} {SYMBOL} at ~{last_price}")
            order_data = LimitOrderRequest(symbol=SYMBOL, qty=qty, limit_price=last_price, side=OrderSide.SELL, time_in_force=TimeInForce.DAY)
            ensure_order(trading_client.submit_order(order_data))
            LAST_ORDER_TS = time.time()
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

def liquidate():
    # Liquidate all SYMBOL positions
    raw_positions = trading_client.get_all_positions()
    for p in raw_positions:
        if isinstance(p, Position) and p.symbol == SYMBOL:
            qty = int(p.qty)
            print(f"[LIQUIDATION] Selling {qty} shares of {SYMBOL}")
            order_data = MarketOrderRequest(
                symbol=SYMBOL,
                qty=qty, 
                side=OrderSide.SELL, 
                time_in_force=TimeInForce.DAY
            )
            trading_client.submit_order(order_data)

def run_stream():
    # seed historical bars so indicators have initial data
    seed_historical(SYMBOL, limit=150)

    # Create a StockDataStream (WebSocket) -- alpaca-py
    if not API_KEY or not API_SECRET:
        raise RuntimeError("Set APCA_API_KEY_ID and APCA_API_SECRET_KEY in .env")
    stream = StockDataStream(API_KEY, API_SECRET)
    try:
        async def handle_bar(bar):
            import pytz
            # Check if past market close time
            et = pytz.timezone('US/Eastern')
            now_et = datetime.now(et).time()
            close_time = datetime.strptime(MARKET_CLOSE_TIME, "%H:%M").time()
            if now_et >= close_time:
                print(f"[MARKET CLOSE] Past {MARKET_CLOSE_TIME} ET - liquidating and exiting")
                liquidate()
                exit(0)

            print(f'received bar: {bar}')
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
