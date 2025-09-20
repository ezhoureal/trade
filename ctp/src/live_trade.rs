use futures::StreamExt;
use std::{any::Any, collections::HashMap, env, thread, time::Duration};

use anyhow::Result;
use async_trait::async_trait;
use backtest::{
    engine::{AccountStatus, Broker, ContractData},
    strategy::PairStrategy,
};
use ctp2rs::ffi::{AssignFromString, WrapToString};
use ctp2rs::v1alpha1::TraderSpiEvent::*;
use ctp2rs::v1alpha1::*;

use crate::{
    market_data::{get_last_price, snapshot_contracts},
    TdAccountConfig,
};

struct LiveBroker {
    api: TraderApi,
    stream: &'static mut TraderSpiStream,
    request_id: i32,
    cash: f32,
    margin: f32,
    positions: HashMap<String, i32>,
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

        let mut pos_net = 0;
        while let Some(spi_msg) = self.stream.next().await {
            match spi_msg {
                TraderSpiEvent::OnRspQryInvestorPosition(event) => {
                    let pos = event.investor_position?;
                    if pos.PosiDirection as u8 == THOST_FTDC_PD_Long {
                        pos_net += pos.Position as i32;
                    } else if pos.PosiDirection as u8 == THOST_FTDC_PD_Short {
                        pos_net -= pos.Position as i32;
                    }
                    println!(
                            "OnRspQryInvestorPosition: contract = {}, position {}, margin = {}, direction = {}", pos.InstrumentID.to_string(),
                            pos.Position, pos.UseMargin, pos.PosiDirection
                        );
                    if event.is_last {
                        if self.positions.contains_key(&instrument)
                            && self.positions[&instrument] != pos_net
                        {
                            println!(
                                "warning: position mismatch for {}, local = {}, exchange = {}",
                                instrument, self.positions[&instrument], pos_net
                            );
                        }
                        self.positions.insert(instrument.clone(), pos_net);
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
                    "OnRspQryTradingAccount: balance={} available={} margin={}, commission = {}, frozen cash = {}, frozen margin = {}",
                    acc.Balance, acc.Available, acc.CurrMargin, acc.Commission, acc.FrozenCash, acc.FrozenMargin
                );
                self.cash = acc.Available as f32;
                self.margin = acc.CurrMargin as f32;
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
        order.OrderPriceType = THOST_FTDC_OPT_AnyPrice as i8;
        order.TimeCondition = THOST_FTDC_TC_IOC as i8;
        order.VolumeCondition = THOST_FTDC_VC_MV as i8;
        order.VolumeTotalOriginal = qty;
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

    async fn submit_order(&mut self, symbol: &str, qty: i32, open: bool) -> Option<i32> {
        let price = get_last_price(symbol)?;
        println!(
            "PLACE ORDER: {:?}, qty = {}, open = {}, price = {}, cost = {}, cash = {}",
            symbol,
            qty,
            open,
            price,
            2.0 * price * qty as f32,
            self.cash
        );
        let mut order = self.order_default(symbol, qty);
        order.LimitPrice = price as f64;
        order.CombOffsetFlag[0] = if open {
            THOST_FTDC_OF_Open as i8
        } else {
            THOST_FTDC_OF_CloseYesterday as i8
        };

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
                    self.cash -= 2.0 * price * volume as f32;
                    return Some(volume);
                }
                _ => {
                    println!("其它回报");
                }
            }
        }
        Some(0)
    }
}

#[async_trait]
impl Broker for LiveBroker {
    async fn exec_spread(&mut self, pair: (&str, &str), mut qty: i32, open: bool) -> Option<u32> {
        // execute the less liquid leg first
        qty = self.submit_order(pair.1, qty, open).await?;
        if qty == 0 {
            return None;
        }
        self.positions
            .entry(pair.1.to_string())
            .and_modify(|e| *e += qty)
            .or_insert(qty);

        let qty2 = self.submit_order(pair.0, -qty, open).await?;
        if qty2 != qty {
            println!(
                "warning: filled qty not match for spread legs: {}, {}",
                qty, qty2
            );
            // maybe attempt to revert first order?
        }
        self.positions
            .entry(pair.0.to_string())
            .and_modify(|e| *e -= qty)
            .or_insert(-qty);
        Some(qty.abs() as u32)
    }

    fn get_status(&'_ self) -> AccountStatus {
        // Mark-to-market valuation using latest prices; unknown prices treated as zero.
        let mut position_value: f32 = 0.0;
        for (sym, size) in self.positions.iter() {
            if let Some(p) = get_last_price(sym) {
                position_value += p * *size as f32;
            }
        }
        // Gross exposure: sum |position| * price
        let mut gross_exposure: f32 = 0.0;
        for (sym, size) in self.positions.iter() {
            if let Some(p) = get_last_price(sym) {
                gross_exposure += p * size.abs() as f32;
            }
        }
        AccountStatus {
            cash: self.cash,
            equity: self.cash + position_value,
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
        positions: HashMap::new(),
        broker_id: env::var("OPENCTP_USER_ID").unwrap_or("".into()),
    }
}

#[tokio::main]
pub async fn run_td(config: TdAccountConfig, strategy: &mut PairStrategy) -> Result<()> {
    let mut broker = init_api(config).await;
    thread::sleep(Duration::from_secs(1));
    for _ in 0..100 {
        let contracts = snapshot_contracts();
        println!("td loop, snapshot has {} contracts", contracts.len());
        // broker.update_positions();
        broker.sync(&contracts.clone()).await;

        // prevent deadlock, as strategy.trade calls broker.exec_spread which is async
        tokio::task::block_in_place(|| {
            strategy.trade(0, contracts.clone(), contracts, &mut broker)
        })?;
        strategy.pop_spread(); // today's spread needs to be replaced

        thread::sleep(Duration::from_secs(10));
    }
    Ok(())
}
