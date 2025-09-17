use futures::StreamExt;
use std::{collections::HashMap, thread, time::Duration};

use anyhow::Result;
use backtest::{
    engine::{AccountStatus, Broker},
    strategy::PairStrategy,
};
use ctp2rs::ffi::{AssignFromString, WrapToString};
use ctp2rs::v1alpha1::TraderSpiEvent::*;
use ctp2rs::v1alpha1::*;

use crate::{
    market_data::{get_last_price, snapshot_contracts},
    TdAccountConfig,
};

/// Very lightweight live broker implementation.
/// Currently this manages a virtual position & cash book locally while
/// delegating eventual real order routing integration to future work.
/// Once order callbacks are wired, the req_order_insert calls (commented)
/// can replace the virtual fills below.
struct LiveBroker {
    api: TraderApi,
    stream: &'static mut TraderSpiStream,
    request_id: i32,
    cash: f32,
    positions: HashMap<String, i32>, // signed position size (+ long / - short)
    broker_id: String,
    investor_id: String,

    pub config: TdAccountConfig,
}

impl LiveBroker {
    async fn sync(&mut self) -> Result<()> {
        for (pos, _) in self.positions.iter() {
            self.request_id += 1;
            let mut qry = CThostFtdcQryInvestorPositionField::default();
            qry.InvestorID.assign_from_str(&self.config.td_user_id);
            qry.BrokerID.assign_from_str(&self.broker_id);
            qry.InstrumentID.assign_from_str(pos);
            self.api
                .req_qry_investor_position(&mut qry, self.request_id);
        }

        while let Some(spi_msg) = self.stream.next().await {
            match spi_msg {
                TraderSpiEvent::OnRspQryInvestorPosition(event) => {
                    if let Some(pos) = event.investor_position {
                        println!(
                            "OnRspQryInvestorPosition {} pos={} today={} yd={}",
                            pos.InstrumentID.to_string(),
                            pos.Position,
                            pos.TodayPosition,
                            pos.YdPosition
                        );
                        self.positions
                            .insert(pos.InstrumentID.to_string(), pos.Position as i32);
                    }
                }
                _ => {
                    println!("td sync got event: {:?}", spi_msg);
                }
            }
        }
        Ok(())
    }
}

impl Broker for LiveBroker {
    fn buy(&mut self, symbol: &str, qty: u32) -> Option<i32> {
        let price = get_last_price(symbol)?; // No price => skip
        let pos_before = *self.positions.get(symbol).unwrap_or(&0);

        // Decide offset flag (open vs close). If existing position is < 0 (short) we are reducing (closing) some or all.
        let closing = pos_before < 0; // buying to cover
        let offset_flag = if closing {
            THOST_FTDC_OF_Close
        } else {
            THOST_FTDC_OF_Open
        };

        self.request_id += 1;
        let mut order = CThostFtdcInputOrderField::default();
        // Mandatory IDs
        order.BrokerID.assign_from_str(&self.broker_id);
        order.InvestorID.assign_from_str(&self.investor_id);
        order.InstrumentID.assign_from_str(symbol);
        // Order reference (user can replace later with something meaningful)
        order.OrderRef.assign_from_str("1");
        // Direction
        order.Direction = THOST_FTDC_D_Buy as i8;
        // Offset (comb) & hedge flags (first char used, rest left '\0')
        order.CombOffsetFlag[0] = offset_flag as i8;
        order.CombHedgeFlag[0] = THOST_FTDC_HF_Speculation as i8;
        // Price & size
        order.LimitPrice = price as f64;
        order.VolumeTotalOriginal = qty as i32;
        // Price type / time & volume conditions
        order.OrderPriceType = THOST_FTDC_OPT_LimitPrice as i8;
        order.TimeCondition = THOST_FTDC_TC_GFD as i8; // GFD
        order.VolumeCondition = THOST_FTDC_VC_AV as i8; // All volume
        order.MinVolume = 1;
        order.ContingentCondition = THOST_FTDC_CC_Immediately as i8;
        order.ForceCloseReason = THOST_FTDC_FCC_NotForceClose as i8;
        order.IsAutoSuspend = 0;
        order.UserForceClose = 0;
        // TODO fields user should fill properly later
        // order.ExchangeID.assign_from_str("todo_exchange");
        // order.InvestUnitID.assign_from_str("todo_invest_unit");
        // order.AccountID.assign_from_str("todo_account");

        let ret = self.api.req_order_insert(&mut order, self.request_id);
        println!("req_order_insert BUY {symbol} qty={qty} price={price} ret={ret} offset={} pos_before={}", if closing {"Close"} else {"Open"}, pos_before);

        // Virtual immediate fill to keep strategy logic working until callbacks adjust real fills.
        let size_entry = self.positions.entry(symbol.to_string()).or_insert(0);
        self.cash -= price * qty as f32;
        *size_entry += qty as i32;
        let new_size = *size_entry;
        if new_size == 0 {
            self.positions.remove(symbol);
        }
        Some(new_size)
    }

    fn sell(&mut self, symbol: &str, qty: u32) -> Option<i32> {
        let price = get_last_price(symbol)?;
        let pos_before = *self.positions.get(symbol).unwrap_or(&0);
        // Selling either opens (if pos <=0) or closes long
        let closing = pos_before > 0; // selling to reduce long
        let offset_flag = if closing {
            THOST_FTDC_OF_Close
        } else {
            THOST_FTDC_OF_Open
        };

        self.request_id += 1;
        let mut order = CThostFtdcInputOrderField::default();
        order.BrokerID.assign_from_str(&self.broker_id);
        order.InvestorID.assign_from_str(&self.investor_id);
        order.InstrumentID.assign_from_str(symbol);
        order.OrderRef.assign_from_str("1");
        order.Direction = THOST_FTDC_D_Sell as i8;
        order.CombOffsetFlag[0] = offset_flag as i8;
        order.CombHedgeFlag[0] = THOST_FTDC_HF_Speculation as i8;
        order.LimitPrice = price as f64;
        order.VolumeTotalOriginal = qty as i32;
        order.OrderPriceType = THOST_FTDC_OPT_LimitPrice as i8;
        order.TimeCondition = THOST_FTDC_TC_GFD as i8;
        order.VolumeCondition = THOST_FTDC_VC_AV as i8;
        order.MinVolume = 1;
        order.ContingentCondition = THOST_FTDC_CC_Immediately as i8;
        order.ForceCloseReason = THOST_FTDC_FCC_NotForceClose as i8;
        order.IsAutoSuspend = 0;
        order.UserForceClose = 0;
        // TODO: set ExchangeID / other account-related fields as needed.
        let ret = self.api.req_order_insert(&mut order, self.request_id);
        println!("req_order_insert SELL {symbol} qty={qty} price={price} ret={ret} offset={} pos_before={}", if closing {"Close"} else {"Open"}, pos_before);

        // Virtual immediate fill
        let size_entry = self.positions.entry(symbol.to_string()).or_insert(0);
        self.cash += price * qty as f32;
        *size_entry -= qty as i32;
        let new_size = *size_entry;
        if new_size == 0 {
            self.positions.remove(symbol);
        }
        Some(new_size)
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
            OnFrontConnected(p) => {
                let mut req = CThostFtdcReqAuthenticateField::default();
                req.UserID.assign_from_str(&config.td_user_id);
                req.AuthCode.assign_from_str(&config.td_auth_code);
                req.AppID.assign_from_str(&config.td_app_id);
                tdapi.req_authenticate(&mut req, 0);
            }
            OnRspAuthenticate(p) => {
                let mut req = CThostFtdcReqUserLoginField::default();
                req.UserID.assign_from_str(&config.td_user_id);
                req.Password.assign_from_str(&config.td_password);
                // // 登录后才能下单
                tdapi.req_user_login(&mut req, 0);
                break;
            }
            _ => {
                println!("td waiting for auth rsp, got event: {:?}", spi_msg);
            }
        }
    }
    println!("td login successful");

    LiveBroker {
        request_id: 0,
        config,
        api: tdapi,
        stream: resp_stream,
        cash: 0.0,
        positions: HashMap::new(),
        broker_id: "9999".into(),
        investor_id: "9991".into(),
    }
}

#[tokio::main]
pub async fn run_td(config: TdAccountConfig, mut strategy: PairStrategy) -> Result<()> {
    let mut broker = init_api(config).await;
    loop {
        println!("td loop");
        // Snapshot all current contracts (single lock read) and iterate
        let contracts = snapshot_contracts();
        println!("td snapshot has {} contracts", contracts.len());
        for c in &contracts {
            println!("td sees {} price={} vol={}", c.name, c.price, c.volume);
        }
        let b = contracts.clone(); // intra-commodity pairs for now
        // broker.update_positions();
        
        broker.sync().await?;
        strategy.trade(0, contracts, b, &mut broker)?;
        strategy.pop_spread();

        thread::sleep(Duration::from_secs(10));
    }
}
