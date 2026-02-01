#!/usr/bin/env python3
"""
Example Python script to demonstrate how to connect to real-time Chinese stock market data
and perform strategy testing using external data sources.

This is a conceptual example showing how you would integrate with real data sources
like Tushare, AkShare, or other Chinese market data providers.
"""

import akshare as ak
import pandas as pd
import numpy as np
from datetime import datetime, timedelta
import time
import sys
import os

def get_real_time_data_akshare(symbols):
    """
    Get real-time or near real-time data from AkShare
    """
    print(f"Fetching data for symbols: {symbols}")
    
    results = []
    for symbol in symbols:
        try:
            # Format symbol for akshare (convert from our format)
            # Our format: "000001.SZ", "600519.SH"
            # AkShare format: "sz000001", "sh600519"
            
            if ".SZ" in symbol:
                ak_symbol = "sz" + symbol.replace(".SZ", "")
            elif ".SH" in symbol:
                ak_symbol = "sh" + symbol.replace(".SH", "")
            else:
                ak_symbol = symbol  # assume already in correct format
            
            # Get stock data
            stock_zh_a_spot_em_df = ak.stock_zh_a_spot_em()
            stock_data = stock_zh_a_spot_em_df[stock_zh_a_spot_em_df['代码'] == ak_symbol.replace('sz', '').replace('sh', '')]
            
            if not stock_data.empty:
                row = stock_data.iloc[0]
                market_data = {
                    'symbol': symbol,
                    'name': row.get('名称', 'Unknown'),
                    'price': float(row.get('最新价', 0)),
                    'open': float(row.get('开盘价', 0)),
                    'high': float(row.get('最高价', 0)),
                    'low': float(row.get('最低价', 0)),
                    'close': float(row.get('昨收', 0)),  # Using yesterday's close as proxy for close
                    'volume': int(row.get('成交量', 0)),
                    'turnover': float(row.get('成交额', 0)),
                    'timestamp': datetime.now().isoformat(),
                    '涨跌幅': float(row.get('涨跌幅', 0)),
                    '涨跌额': float(row.get('涨跌额', 0)),
                    'bid_price': float(row.get('最新价', 0)) * 0.999,  # Simplified bid price
                    'ask_price': float(row.get('最新价', 0)) * 1.001,  # Simplified ask price
                    'bid_volume': int(row.get('成交量', 0)) // 100,  # Simplified bid volume
                    'ask_volume': int(row.get('成交量', 0)) // 100,  # Simplified ask volume
                }
                results.append(market_data)
                print(f"  {symbol}: {market_data['name']} - {market_data['price']}")
            else:
                print(f"  Warning: No data found for {symbol}")
                
        except Exception as e:
            print(f"  Error fetching data for {symbol}: {e}")
    
    return results

class SimulatedExecution:
    """
    Simulated execution engine for testing trading strategies
    """
    def __init__(self, initial_capital=1000000.0):
        self.positions = {}
        self.cash_balance = initial_capital
        self.initial_capital = initial_capital
        self.historical_trades = []
        self.transaction_fee_rate = 0.0003  # 0.03% fee
        self.slippage_rate = 0.0005        # 0.05% slippage
        self.min_transaction_fee = 5.0     # minimum 5 RMB per trade
        
    def execute_order(self, order, market_data):
        """
        Execute an order based on market data
        """
        print(f"Executing order: {order['side']} {order['quantity']} shares of {order['symbol']} at {market_data['price']}")
        
        # Determine execution price based on order type and market conditions
        if order['order_type'] == 'MARKET':
            # For market orders, use ask price for buys, bid price for sells
            if order['side'] == 'BUY':
                execution_price = market_data['ask_price'] * (1 + self.slippage_rate)
            else:
                execution_price = market_data['bid_price'] * (1 - self.slippage_rate)
        elif order['order_type'] == 'LIMIT':
            # For limit orders, check if the limit price is acceptable
            if order['side'] == 'BUY' and order['price'] >= market_data['ask_price']:
                execution_price = market_data['ask_price'] * (1 + self.slippage_rate * 0.5)
            elif order['side'] == 'SELL' and order['price'] <= market_data['bid_price']:
                execution_price = market_data['bid_price'] * (1 - self.slippage_rate * 0.5)
            else:
                # Limit not reached
                print(f"  Limit order not executed: {order['price']} vs market")
                return None
        else:
            execution_price = market_data['price'] * (1 + self.slippage_rate if order['side'] == 'BUY' else 1 - self.slippage_rate)

        # Calculate total cost
        gross_amount = execution_price * order['quantity']
        transaction_fee = max(gross_amount * self.transaction_fee_rate, self.min_transaction_fee)
        total_amount = gross_amount + transaction_fee if order['side'] == 'BUY' else gross_amount - transaction_fee

        # Validate funds for buy orders
        if order['side'] == 'BUY' and total_amount > self.cash_balance:
            print(f"  Insufficient funds: need {total_amount:.2f}, have {self.cash_balance:.2f}")
            return None

        # Validate position for sell orders
        if order['side'] == 'SELL':
            if order['symbol'] in self.positions:
                if order['quantity'] > self.positions[order['symbol']]['available']:
                    print(f"  Insufficient shares to sell: need {order['quantity']}, available {self.positions[order['symbol']]['available']}")
                    return None
            else:
                print(f"  No position found for symbol: {order['symbol']}")
                return None

        # Execute the trade
        if order['side'] == 'BUY':
            self.cash_balance -= total_amount

            # Update or create position
            if order['symbol'] not in self.positions:
                self.positions[order['symbol']] = {
                    'symbol': order['symbol'],
                    'name': market_data['name'],
                    'quantity': 0,
                    'available': 0,
                    'avg_cost': 0.0,
                    'market_value': 0.0,
                    'profit_loss': 0.0,
                }

            position = self.positions[order['symbol']]
            old_quantity = position['quantity']
            old_cost = position['avg_cost'] * old_quantity
            new_quantity = old_quantity + order['quantity']
            new_cost = old_cost + gross_amount

            position['quantity'] = new_quantity
            position['available'] = new_quantity  # For simplicity, assume immediate settlement
            position['avg_cost'] = new_cost / new_quantity if new_quantity > 0 else 0.0
            position['market_value'] = execution_price * new_quantity
        elif order['side'] == 'SELL':
            self.cash_balance += total_amount

            if order['symbol'] in self.positions:
                position = self.positions[order['symbol']]
                position['quantity'] -= order['quantity']
                position['available'] -= order['quantity']

                # Update market value
                position['market_value'] = execution_price * position['quantity']
                
                # Update profit/loss calculation
                sold_cost = position['avg_cost'] * order['quantity']
                sold_revenue = execution_price * order['quantity']
                position['profit_loss'] += sold_revenue - sold_cost

        # Record the trade
        trade_record = {
            'order_id': f"TRADE_{int(time.time())}_{np.random.randint(1000, 9999)}",
            'symbol': order['symbol'],
            'side': order['side'],
            'quantity': order['quantity'],
            'price': execution_price,
            'timestamp': datetime.now().isoformat(),
            'fee': transaction_fee,
        }

        self.historical_trades.append(trade_record)

        order_response = {
            'order_id': f"ORDER_{int(time.time())}_{np.random.randint(1000, 9999)}",
            'status': 'FILLED',
            'symbol': order['symbol'],
            'filled_quantity': order['quantity'],
            'avg_fill_price': execution_price,
        }

        print(f"  Order executed successfully: {order_response}")
        return order_response

    def get_performance_metrics(self):
        """Calculate performance metrics"""
        current_value = self.cash_balance
        for position in self.positions.values():
            current_value += position['market_value']
        
        pnl = current_value - self.initial_capital
        return_rate = (pnl / self.initial_capital) * 100.0

        return {
            'total_return': return_rate,
            'total_pnl': pnl,
            'current_portfolio_value': current_value,
            'total_trades': len(self.historical_trades),
            'cash_balance': self.cash_balance,
        }

def simple_moving_average_strategy(market_data, execution_engine, short_window=5, long_window=20):
    """
    Simple moving average crossover strategy example
    """
    # This is a placeholder - in a real implementation, you'd need historical data
    # to calculate moving averages
    orders = []
    
    for data in market_data:
        # Example logic: if we have no position and price is favorable, buy
        # If we have a position and certain conditions are met, sell
        symbol = data['symbol']
        
        if symbol not in execution_engine.positions or execution_engine.positions[symbol]['quantity'] == 0:
            # Consider buying if price looks attractive (this is simplified)
            if data['涨跌幅'] is not None and data['涨跌幅'] < -2:  # If stock dropped more than 2%
                order = {
                    'symbol': symbol,
                    'side': 'BUY',
                    'quantity': 1000,  # Fixed quantity for example
                    'price': data['ask_price'],
                    'order_type': 'LIMIT',
                    'entrust_type': 'E'
                }
                orders.append((order, data))
        else:
            # Consider selling if position has gained significantly
            position = execution_engine.positions[symbol]
            current_profit = (data['price'] - position['avg_cost']) / position['avg_cost'] * 100
            if current_profit > 5:  # Take profit at 5% gain
                order = {
                    'symbol': symbol,
                    'side': 'SELL',
                    'quantity': min(500, position['available']),  # Sell half the position
                    'price': data['bid_price'],
                    'order_type': 'LIMIT',
                    'entrust_type': 'E'
                }
                orders.append((order, data))
    
    return orders

def main():
    print("Chinese Stock Market Real-Time Strategy Testing")
    print("="*50)
    
    # Initialize simulated execution engine
    execution_engine = SimulatedExecution(initial_capital=1000000.0)  # 1 million RMB
    print(f"Initialized with {execution_engine.initial_capital:,.2f} RMB")
    
    # Define symbols to watch
    symbols = ["000001.SZ", "600519.SH", "002594.SZ"]  # Ping An Bank, Kweichow Moutai, BYD
    
    print("\nStarting real-time strategy testing...")
    print("Symbols:", symbols)
    
    # Run for a few iterations to simulate real-time testing
    for iteration in range(5):  # Run 5 iterations for demonstration
        print(f"\n--- Iteration {iteration + 1} ---")
        
        # Get real-time market data
        print("Fetching real-time market data...")
        market_data_list = get_real_time_data_akshare(symbols)
        
        if not market_data_list:
            print("No market data received, skipping iteration")
            time.sleep(2)
            continue
        
        # Apply trading strategy
        print("Applying trading strategy...")
        strategy_orders = simple_moving_average_strategy(market_data_list, execution_engine)
        
        # Execute orders
        for order, market_data in strategy_orders:
            print(f"Executing strategy order: {order['side']} {order['quantity']} {order['symbol']}")
            execution_engine.execute_order(order, market_data)
        
        # Print current performance
        metrics = execution_engine.get_performance_metrics()
        print(f"Performance Metrics:")
        print(f"  Total P&L: {metrics['total_pnl']:,.2f} RMB ({metrics['total_return']:+.2f}%)")
        print(f"  Portfolio Value: {metrics['current_portfolio_value']:,.2f} RMB")
        print(f"  Cash Balance: {metrics['cash_balance']:,.2f} RMB")
        print(f"  Total Trades: {metrics['total_trades']}")
        
        # Print current positions
        if execution_engine.positions:
            print("  Current Positions:")
            for symbol, pos in execution_engine.positions.items():
                if pos['quantity'] > 0:
                    print(f"    {pos['name']} ({symbol}): {pos['quantity']} shares @ {pos['avg_cost']:.2f}, P&L: {pos['profit_loss']:.2f} RMB")
        
        print(f"Waiting 10 seconds until next iteration...")
        time.sleep(10)  # Wait 10 seconds before next iteration
    
    print(f"\nFinal Results after {len(execution_engine.historical_trades)} trades:")
    final_metrics = execution_engine.get_performance_metrics()
    print(f"  Final Portfolio Value: {final_metrics['current_portfolio_value']:,.2f} RMB")
    print(f"  Total Return: {final_metrics['total_return']:+.2f}%")
    print(f"  Total P&L: {final_metrics['total_pnl']:,.2f} RMB")

if __name__ == "__main__":
    # Check if akshare is available
    try:
        import akshare
        print("AkShare is available. You can use this script to connect to real market data.")
    except ImportError:
        print("AkShare not installed. Install it with: pip install akshare")
        print("This script demonstrates how to connect to real market data sources.")
        sys.exit(1)
    
    main()