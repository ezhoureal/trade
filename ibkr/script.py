from ib_insync import *

ib = IB()
ib.connect('127.0.0.1', 7497, clientId=1)

print(f'Connected. Account status: {ib.accountSummary()}')

# 1. Define the underlying
symbol = 'NVDA'
option = Option(symbol, '20251010', 190, 'C', 'SMART')
order = MarketOrder('BUY', 1)  # Buy 1 call at market price
trade = ib.placeOrder(option, order)

# 5. Monitor order status
ib.sleep(2)
print(trade.orderStatus.status)
