1. excel -> parquet
2. calculate co-integration of every pair of contract
3. choose valid pairs
4. back test statistical arbitrage strategy on all valid pairs and select top performers

## Discovered Pair Strategies

### Top Performing Strategies from Back-testing Analysis

#### 1. **Base Metals Cross-Pair Strategy** ⭐ HIGHEST RETURNS
- **Cu-Fu (Copper-Fuel Oil)**: Avg return 0.815, Win rate 81.2%
- **Al-Ni (Aluminum-Nickel)**: Avg return 2.702, Sharpe 48.6
- **Ag-Ni (Silver-Nickel)**: Avg return 2.438, mixed sectors advantage
- **Strategy**: Trade seasonal and supply-demand divergences between related base metals
- **Key insight**: Nickel appears in 58 profitable pairs (most volatile, best mean-reversion)

#### 2. **Base Metals vs Energy Strategy** ⭐ HIGHEST WIN RATE
- **Performance**: 51 profitable pairs with 77% average win rate
- **Top pairs**: Fu-Ni, Cu-Fu, Bu-Ni (Bitumen-Nickel)
- **Strategy**: Capitalize on industrial metals vs energy cost correlations
- **Parameters**: 20-day lookback, ±2.0σ entry, ±0.5σ exit

#### 3. **Copper-Centric Strategy** ⭐ MOST CONSISTENT
- **Frequency**: Copper in 92 profitable pairs (most frequent winner)
- **Best combinations**: Cu-Fu, Cu-Sp (Pulp), Cu-Rb (Rebar), Cu-Hc (Hot Rolled Coil)
- **Win rates**: Often 80%+ for Cu-Fu pairs
- **Strategy**: Copper as economic indicator, trades well against energy & soft commodities

#### 4. **Calendar Spread Strategy**
- **Same commodity, different months**: Ni2403-Ni2411 (2.996 return), Sn2403-Sn2405 (2.015 return)
- **Strategy**: Exploit contango/backwardation in futures curves
- **Advantage**: Lower correlation risk, pure time-decay arbitrage

#### 5. **Steel Complex Internal Arbitrage**
- **Pairs**: Rb-Hc (Rebar-Hot Rolled Coil), Ss-Wr (Stainless Steel-Wire Rod)
- **Strategy**: Trade price differentials within steel production chain
- **Profile**: Lower volatility but consistent risk-adjusted returns

### Key Strategic Insights
1. **Cross-sector pairs outperform intra-sector** (Base Metals-Energy: 77% win rate)
2. **Nickel is the "golden commodity"** (highest volatility, best mean-reversion)
3. **High win rate vs high return trade-off** (Energy pairs: high win%, Precious metals: high return)
4. **Industrial correlation dominance** (Cu pairs consistently profitable)
5. **Optimal parameters**: 20-day window, ±2σ entry, ±0.5σ exit

### Sector Performance Ranking
1. **Base Metals - Precious Metals**: 2.186 avg return (12 pairs)
2. **Base Metals (intra-sector)**: 1.598 avg return (22 pairs) 
3. **Base Metals - Energy**: 1.155 avg return, 77% win rate (51 pairs)
4. **Base Metals - Soft Commodities**: 1.117 avg return, 61.8% win rate (31 pairs)
5. **Base Metals - Steel**: 0.985 avg return, 65.6% win rate (47 pairs)