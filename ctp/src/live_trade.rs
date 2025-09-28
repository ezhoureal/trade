use chrono::{NaiveDate, ParseResult};
use futures::StreamExt;
use std::{any::Any, collections::HashMap, env, thread, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use backtest::{
    engine::{AccountStatus, Broker, ContractData, PositionKind},
    strategy::{PairStrategy, SpreadPositions},
};
use ctp2rs::ffi::{AssignFromString, WrapToString};
use ctp2rs::v1alpha1::TraderSpiEvent::*;
use ctp2rs::v1alpha1::*;

use crate::{
    market_data::{get_price, get_volume, snapshot_contracts},
    TdAccountConfig,
};

#[derive(Clone, Debug)]
struct Position {
    long_today: u32,
    long_yd: u32,
    short_today: u32,
    short_yd: u32,
}

impl Position {
    fn default() -> Self {
        Position {
            long_today: 0,
            long_yd: 0,
            short_today: 0,
            short_yd: 0,
        }
    }
}

struct LiveBroker {
    api: TraderApi,
    stream: &'static mut TraderSpiStream,
    request_id: i32,
    date: NaiveDate,
    instrument_expiry: HashMap<String, NaiveDate>,
    cash: f32,
    margin: f32,
    equity: f32,
    positions: HashMap<String, Position>,
    broker_id: String,
    pub config: TdAccountConfig,
}

impl LiveBroker {
    async fn sync_position(&mut self, contract: &ContractData) -> Option<()> {
        self.request_id += 1;
        let instrument = contract.name.clone();
        let mut qry = CThostFtdcQryInvestorPositionField::default();
        qry.InvestorID.assign_from_str(&self.config.td_user_id);
        qry.InstrumentID.assign_from_str(&instrument);
        self.api
            .req_qry_investor_position(&mut qry, self.request_id);

        let mut long_today = 0;
        let mut short_today = 0;
        let mut long_yd = 0;
        let mut short_yd = 0;
        while let Some(spi_msg) = self.stream.next().await {
            match spi_msg {
                TraderSpiEvent::OnRspQryInvestorPosition(event) => {
                    let pos = event.investor_position?;
                    if pos.PosiDirection as u8 == THOST_FTDC_PD_Long {
                        long_today += pos.TodayPosition as u32;
                        long_yd += (pos.Position - pos.TodayPosition) as u32;
                    } else if pos.PosiDirection as u8 == THOST_FTDC_PD_Short {
                        short_today += pos.TodayPosition as u32;
                        short_yd += (pos.Position - pos.TodayPosition) as u32;
                    }
                    println!(
                            "OnRspQryInvestorPosition: contract = {}, position {}, ydPosition {}, todayPosition {} margin = {}, direction = {}", pos.InstrumentID.to_string(),
                            pos.Position, pos.YdPosition, pos.TodayPosition, pos.UseMargin, pos.PosiDirection
                        );
                    if event.is_last {
                        self.positions.insert(
                            instrument.clone(),
                            Position {
                                long_today,
                                long_yd,
                                short_today,
                                short_yd,
                            },
                        );
                        break;
                    }
                }
                _ => {
                    println!(
                        "unexpected event while syncing position: {:?}",
                        spi_msg.type_id()
                    );
                }
            }
        }
        Some(())
    }

    async fn sync(&mut self, contracts: &[ContractData]) -> Option<()> {
        self.request_id += 1;
        let mut qry = CThostFtdcQryTradingAccountField::default();
        qry.InvestorID.assign_from_str(&self.config.td_user_id);
        self.api.req_qry_trading_account(&mut qry, self.request_id);
        let spi_msg = self.stream.next().await?;
        if let TraderSpiEvent::OnRspQryTradingAccount(event) = spi_msg {
            if let Some(acc) = event.trading_account {
                println!(
                    "OnRspQryTradingAccount: trading day = {}, balance={} available={} margin={}, commission = {}, frozen cash = {}, frozen margin = {}",
                    acc.TradingDay.to_string(), acc.Balance, acc.Available, acc.CurrMargin, acc.Commission, acc.FrozenCash, acc.FrozenMargin
                );
                self.date =
                    NaiveDate::parse_from_str(&acc.TradingDay.to_string(), "%Y%m%d").ok()?;
                self.cash = acc.Available as f32;
                self.margin = acc.CurrMargin as f32;
                self.equity = acc.Balance as f32;
            } else if let Some(err) = event.rsp_info {
                println!(
                    "OnRspQryTradingAccount: error message = {:?}",
                    err.ErrorMsg.to_string()
                );
            }
        }
        for contract in contracts.iter() {
            self.sync_position(contract).await;
        }
        Some(())
    }

    fn order_default(&self, symbol: &str, qty: i32) -> CThostFtdcInputOrderField {
        let mut order = CThostFtdcInputOrderField::default();
        order.InvestorID.assign_from_str(&self.config.td_user_id);
        order.UserID.assign_from_str(&self.config.td_user_id);
        order.BrokerID.assign_from_str(&self.broker_id);
        order.ExchangeID.assign_from_str("SHFE");
        order.InstrumentID.assign_from_str(symbol);
        order.CombHedgeFlag[0] = THOST_FTDC_HF_Speculation as i8;
        order.OrderPriceType = THOST_FTDC_OPT_LimitPrice as i8;
        if self.config.special_close_all {
            order.OrderPriceType = THOST_FTDC_OPT_AnyPrice as i8;
        }
        order.TimeCondition = THOST_FTDC_TC_GFD as i8;
        order.VolumeCondition = THOST_FTDC_VC_MV as i8;
        order.VolumeTotalOriginal = qty.abs();
        order
            .OrderRef
            .assign_from_str(self.request_id.to_string().as_str());
        order.Direction = if qty > 0 {
            THOST_FTDC_D_Buy as i8
        } else {
            THOST_FTDC_D_Sell as i8
        };
        order
    }

    #[allow(dead_code)]
    async fn close_all(&mut self) -> Option<()> {
        self.sync(&snapshot_contracts()).await?;
        println!(
            "closing all positions, position size = {}",
            self.positions.len()
        );
        let positions = self.positions.clone();
        for (symbol, pos) in positions.iter() {
            println!(
                "closing position for {}: long_today = {}, long_yd = {}, short_today = {}, short_yd = {}",
                symbol, pos.long_today, pos.long_yd, pos.short_today, pos.short_yd
            );
            self.submit_order(symbol, -(pos.long_yd as i32), THOST_FTDC_OF_CloseYesterday)
                .await?;
            self.submit_order(&symbol, -(pos.long_today as i32), THOST_FTDC_OF_CloseToday)
                .await?;
            self.submit_order(&symbol, pos.short_yd as i32, THOST_FTDC_OF_CloseYesterday)
                .await?;
            self.submit_order(&symbol, pos.short_today as i32, THOST_FTDC_OF_CloseToday)
                .await?;
        }
        self.sync(&snapshot_contracts()).await?;
        Some(())
    }

    async fn submit_order(&mut self, symbol: &str, qty: i32, offset_flag: u8) -> Option<i32> {
        if qty == 0 {
            return Some(0);
        }
        let price = get_price(symbol)?;
        println!(
            "PLACE ORDER: {:?}, qty = {}, open = {}, price = {}, cash = {}",
            symbol,
            qty,
            offset_flag == THOST_FTDC_OF_Open,
            price,
            self.cash
        );
        let mut order = self.order_default(symbol, qty);
        order.LimitPrice = price as f64;
        order.CombOffsetFlag[0] = offset_flag as i8;

        self.request_id += 1;
        self.api.req_order_insert(&mut order, self.request_id);

        while let Some(spi_msg) = self.stream.next().await {
            match spi_msg {
                OnRtnOrder(p) => {
                    let order = p.order.unwrap();
                    let broker_id = order.BrokerID.to_string();
                    let investor_id = order.InvestorID.to_string();
                    let exchange_id = order.ExchangeID.to_string();
                    let order_ref = order.OrderRef.to_string();
                    let instrument_id = order.InstrumentID.to_string();

                    let order_status = match order.OrderStatus as u8 {
                        THOST_FTDC_OST_AllTraded => "全部成交",
                        THOST_FTDC_OST_PartTradedQueueing => "部分成交还在队列中",
                        THOST_FTDC_OST_PartTradedNotQueueing => "部分成交不在队列中",
                        THOST_FTDC_OST_NoTradeQueueing => "未成交还在队列中",
                        THOST_FTDC_OST_NoTradeNotQueueing => "未成交不在队列中",
                        THOST_FTDC_OST_Canceled => "已撤销",
                        THOST_FTDC_OST_Unknown => "未知状态",
                        THOST_FTDC_OST_NotTouched => "尚未触发",
                        THOST_FTDC_OST_Touched => "已触发",
                        _ => "其他状态",
                    };

                    println!("报单成功回报 Order Return: BrokerID: {}, InvestorID: {}, ExchangeID: {}, OrderRef: {}, OrderStatus: {}, InstrumentID: {}", 
                          broker_id, investor_id, exchange_id, order_ref, order_status, instrument_id);
                    if order_status == "已撤销" {
                        return None;
                    }
                }
                OnRspOrderInsert(p) => {
                    let rsp_info = p.rsp_info.unwrap();
                    println!(
                        "报单失败回报 OnRspOrderInsert {}: {}",
                        rsp_info.ErrorID,
                        rsp_info.ErrorMsg.to_string(),
                    );
                    return None;
                }
                OnRtnTrade(p) => {
                    let trade = p.trade.unwrap();

                    let broker_id = trade.BrokerID.to_string();
                    let investor_id = trade.InvestorID.to_string();
                    let exchange_id = trade.ExchangeID.to_string();
                    let trade_id = trade.TradeID.to_string();
                    let order_ref = trade.OrderRef.to_string();
                    let instrument_id = trade.InstrumentID.to_string();
                    let price = trade.Price as f32;
                    let volume = trade.Volume;

                    println!("成交回报 OnRtnTrade: OrderRef: {}, BrokerID: {}, InvestorID: {}, ExchangeID: {}, TradeID: {}, InstrumentID: {}, Price: {:.2}, Volume: {}",
                          order_ref, broker_id, investor_id, exchange_id, trade_id, instrument_id, price, volume);
                    return Some(volume);
                }
                _ => {
                    println!("其它回报");
                }
            }
        }
        Some(0)
    }

    async fn query_expiry_date(&mut self, instrument: &str) -> ParseResult<()> {
        let mut qry = CThostFtdcQryInstrumentField::default();
        qry.InstrumentID.assign_from_str(&instrument);
        self.api.req_qry_instrument(&mut qry, self.request_id);
        while let Some(spi_msg) = self.stream.next().await {
            match spi_msg {
                TraderSpiEvent::OnRspQryInstrument(event) => {
                    if let Some(inst) = event.instrument {
                        println!(
                            "OnRspQryInstrument: contract = {}, expire date = {}, price_tick = {}, volume_multiple = {}, exchange = {}",
                            inst.InstrumentID.to_string(),
                            inst.ExpireDate.to_string(),
                            inst.PriceTick,
                            inst.VolumeMultiple,
                            inst.ExchangeID.to_string()
                        );
                        self.instrument_expiry.insert(
                            instrument.to_string(),
                            NaiveDate::parse_from_str(&inst.ExpireDate.to_string(), "%Y%m%d")?,
                        );
                    }
                    if event.is_last {
                        break;
                    }
                }
                _ => {
                    println!(
                        "unexpected event while syncing instrument: {:?}",
                        spi_msg.type_id()
                    );
                }
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    async fn query_commission(&mut self, instrument: &str) {
        let mut qry = CThostFtdcQryInstrumentCommissionRateField::default();
        qry.InvestorID.assign_from_str(&self.config.td_user_id);
        qry.InstrumentID.assign_from_str(&instrument);
        self.api
            .req_qry_instrument_commission_rate(&mut qry, self.request_id);
        while let Some(spi_msg) = self.stream.next().await {
            match spi_msg {
                TraderSpiEvent::OnRspQryInstrumentCommissionRate(event) => {
                    if let Some(rate) = event.instrument_commission_rate {
                        println!(
                            "OnRspQryInstrumentCommissionRate: contract = {}, open_ratio = {}, close_ratio = {}, close_yesterday_ratio = {}",
                            rate.InstrumentID.to_string(),
                            rate.OpenRatioByMoney, rate.CloseRatioByMoney, rate.CloseTodayRatioByMoney
                        );
                    } else if let Some(err) = event.rsp_info {
                        println!(
                            "OnRspQryInstrumentCommissionRate: error message = {:?}",
                            err.ErrorMsg.to_string()
                        );
                    }
                    if event.is_last {
                        break;
                    }
                }
                _ => {
                    println!(
                        "unexpected event while syncing commission rate: {:?}",
                        spi_msg.type_id()
                    );
                }
            }
        }
    }
}

fn determine_flag(open: bool, qty: i32, pos: &Position) -> Option<u8> {
    let long = qty > 0;
    let qty_abs = qty.abs() as u32;
    if open {
        Some(THOST_FTDC_OF_Open)
    } else if long && pos.short_today >= qty_abs {
        Some(THOST_FTDC_OF_CloseToday)
    } else if long && pos.short_yd >= qty_abs {
        Some(THOST_FTDC_OF_CloseYesterday)
    } else if !long && pos.long_today >= qty_abs {
        Some(THOST_FTDC_OF_CloseToday)
    } else if !long && pos.long_yd >= qty_abs {
        Some(THOST_FTDC_OF_CloseYesterday)
    } else {
        println!("no position to close for given qty {}", qty);
        None // fallback to open
    }
}

#[async_trait]
impl Broker for LiveBroker {
    async fn exec_spread(
        &mut self,
        pair: (String, String),
        qty_a: i32,
        qty_b: i32,
        open: bool,
    ) -> Option<(u32, u32)> {
        // execute the less liquid leg
        if get_volume(&pair.0)? > get_volume(&pair.1)? {
            return self.exec_spread((pair.1, pair.0), qty_b, qty_a, open).await;
        }
        let flag = determine_flag(
            open,
            qty_a,
            &self
                .positions
                .entry(pair.0.clone())
                .or_insert(Position::default()),
        )?;
        let qty1 = self.submit_order(&pair.0, qty_a, flag).await?;
        if qty1 == 0 {
            return None;
        }

        // scale down qty_b according to the actual qty1 executed
        let scaled_qty_b = (qty_b as f64 * qty1 as f64 / qty_a.abs() as f64).round() as i32;
        let qty2 = self.submit_order(&pair.1, scaled_qty_b, flag).await?;
        if qty2 != qty1 {
            println!(
                "warning: filled qty not match for spread legs: {}, {}",
                qty1, qty2
            );
            // maybe attempt to revert first order?
        }
        Some((qty1.abs() as u32, qty2.abs() as u32))
    }

    fn get_status(&'_ self) -> AccountStatus {
        // Gross exposure: sum |position| * price
        let mut gross_exposure: f32 = 0.0;
        for (sym, pos) in self.positions.iter() {
            if let Some(p) = get_price(sym) {
                gross_exposure += p * (pos.long_today + pos.short_today) as f32;
            }
        }
        AccountStatus {
            cash: self.cash,
            equity: self.equity,
            gross_exposure,
        }
    }
}

async fn init_api(config: TdAccountConfig) -> LiveBroker {
    println!(
        "td dynlib_path: {}",
        config.td_dynlib_path.to_string_lossy()
    );

    #[cfg(not(feature = "ctp_v6_7_11"))]
    let tdapi = TraderApi::create_api(&config.td_dynlib_path, "./td_");

    #[cfg(feature = "ctp_v6_7_11")]
    let tdapi = TraderApi::create_api(&config.td_dynlib_path, "./td_", true);

    let front_address = config.td_front_address.clone();
    println!("td get_api_version: {}", tdapi.get_api_version());

    tdapi.register_front(&front_address);

    let inner = TraderSpiInner::new();
    let resp_stream = Box::leak(Box::new(TraderSpiStream::new(inner)));
    tdapi.register_spi(resp_stream as *mut dyn TraderSpi);

    tdapi.subscribe_private_topic(THOST_TE_RESUME_TYPE::THOST_TERT_QUICK);
    tdapi.subscribe_public_topic(THOST_TE_RESUME_TYPE::THOST_TERT_QUICK);

    tdapi.init();

    while let Some(spi_msg) = resp_stream.next().await {
        match spi_msg {
            OnFrontConnected(_) => {
                println!("td OnFrontConnected");
                let mut req = CThostFtdcReqAuthenticateField::default();
                req.UserID.assign_from_str(&config.td_user_id);
                req.AuthCode.assign_from_str(&config.td_auth_code);
                req.AppID.assign_from_str(&config.td_app_id);
                tdapi.req_authenticate(&mut req, 0);
            }
            OnRspAuthenticate(_) => {
                let mut req = CThostFtdcReqUserLoginField::default();
                req.UserID.assign_from_str(&config.td_user_id);
                req.Password.assign_from_str(&config.td_password);
                tdapi.req_user_login(&mut req, 0);
            }
            OnRspUserLogin(_) => {
                println!("td OnRspUserLogin");
                break;
            }
            _ => {
                println!("td waiting for auth rsp, got event: {:?}", spi_msg);
            }
        }
    }

    LiveBroker {
        request_id: 0,
        config,
        api: tdapi,
        stream: resp_stream,
        cash: 0.0,
        margin: 0.0,
        equity: 0.0,
        date: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
        instrument_expiry: HashMap::new(),
        positions: HashMap::new(),
        broker_id: env::var("OPENCTP_USER_ID").unwrap_or("".into()),
    }
}

fn check_position_consistency(broker: &HashMap<String, Position>, strategy: &SpreadPositions) {
    let mut total_long = HashMap::new();
    let mut total_short = HashMap::new();
    for ((a, b), pos) in strategy.iter() {
        let (long_sym, short_sym) = match pos.kind {
            PositionKind::Long => (a, b),
            PositionKind::Short => (b, a),
        };
        *total_long.entry(long_sym).or_insert(0) += pos.size_a;
        *total_short.entry(short_sym).or_insert(0) += pos.size_a;
    }
    for (sym, pos) in broker.iter() {
        let long = total_long.get(sym).cloned().unwrap_or(0);
        let short = total_short.get(sym).cloned().unwrap_or(0);
        let broker_long = pos.long_today + pos.long_yd;
        let broker_short = pos.short_today + pos.short_yd;
        if long != broker_long {
            println!(
                "position mismatch for {}: strategy long = {}, broker long = {}",
                sym, long, broker_long
            );
        }
        if short != broker_short {
            println!(
                "position mismatch for {}: strategy short = {}, broker short = {}",
                sym, short, broker_short
            );
        }
    }
}

#[tokio::main]
pub async fn run_td(config: TdAccountConfig, strategy: &mut PairStrategy) -> Result<()> {
    let mut broker = init_api(config).await;
    thread::sleep(Duration::from_secs(1));
    if broker.config.special_close_all {
        broker.close_all().await;
        std::fs::remove_file("positions.json")?;
        return Ok(());
    }

    let contracts = snapshot_contracts();
    println!("syncing positions for {} contracts", contracts.len());
    broker.sync(&contracts).await;
    for contract in contracts.iter() {
        broker.query_expiry_date(&contract.name).await?;
    }
    strategy.set_expiry_dates(&broker.date, &broker.instrument_expiry);

    loop {
        check_position_consistency(&broker.positions, &strategy.get_positions());

        let contracts = snapshot_contracts();
        // prevent deadlock, as strategy.trade calls broker.exec_spread which is async
        tokio::task::block_in_place(|| {
            strategy.trade(0, contracts.clone(), contracts.clone(), &mut broker)
        })?;
        strategy.pop_spread(); // today's spread needs to be replaced

        strategy.save_positions(); // save spread info to disk
        thread::sleep(Duration::from_secs(10));
        broker.sync(&contracts).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(l_today: u32, l_yd: u32, s_today: u32, s_yd: u32) -> Position {
        Position {
            long_today: l_today,
            long_yd: l_yd,
            short_today: s_today,
            short_yd: s_yd,
        }
    }

    #[test]
    fn test_open_returns_open() {
        let p = pos(0, 0, 0, 0);
        assert_eq!(determine_flag(true, 5, &p), Some(THOST_FTDC_OF_Open));
        assert_eq!(determine_flag(true, -3, &p), Some(THOST_FTDC_OF_Open));
    }

    #[test]
    fn test_close_short_prefers_today_then_yd() {
        // buying qty>0 closes short side
        let p_today = pos(0, 0, 4, 1);
        assert_eq!(
            determine_flag(false, 3, &p_today),
            Some(THOST_FTDC_OF_CloseToday)
        );

        let p_yd = pos(0, 0, 0, 6);
        assert_eq!(
            determine_flag(false, 5, &p_yd),
            Some(THOST_FTDC_OF_CloseYesterday)
        );

        let p_both = pos(0, 0, 2, 2);
        // if qty fits in today, choose today
        assert_eq!(
            determine_flag(false, 2, &p_both),
            Some(THOST_FTDC_OF_CloseToday)
        );
        // if qty doesn't fit in either bucket alone, return None (caller should split)
        assert_eq!(determine_flag(false, 3, &p_both), None);
    }

    #[test]
    fn test_close_long_prefers_today_then_yd() {
        // selling qty<0 closes long side
        let p_today = pos(5, 1, 0, 0);
        assert_eq!(
            determine_flag(false, -4, &p_today),
            Some(THOST_FTDC_OF_CloseToday)
        );

        let p_yd = pos(0, 7, 0, 0);
        assert_eq!(
            determine_flag(false, -6, &p_yd),
            Some(THOST_FTDC_OF_CloseYesterday)
        );

        let p_both = pos(2, 3, 0, 0);
        // if qty fits in today, choose today
        assert_eq!(
            determine_flag(false, -2, &p_both),
            Some(THOST_FTDC_OF_CloseToday)
        );
        // if qty doesn't fit in today but fits in yd, choose yd
        assert_eq!(
            determine_flag(false, -3, &p_both),
            Some(THOST_FTDC_OF_CloseYesterday)
        );
    }

    #[test]
    fn test_insufficient_position_returns_none() {
        // buy to close short, but no short positions
        let p = pos(2, 3, 0, 0);
        assert_eq!(determine_flag(false, 1, &p), None);

        // sell to close long, but no long positions
        let p2 = pos(0, 0, 4, 5);
        assert_eq!(determine_flag(false, -1, &p2), None);
    }
}
