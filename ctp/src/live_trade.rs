#![allow(unused_variables)]
use std::{collections::HashMap, sync::Arc, thread, time::Duration};

use backtest::{
    engine::{AccountStatus, Broker},
    params::Params,
    strategy::PairStrategy,
};
use ctp2rs::{
    ffi::{gb18030_cstr_i8_to_str, AssignFromString, WrapToString},
    print_rsp_info,
    v1alpha1::{
        CThostFtdcInputOrderField, CThostFtdcInvestorPositionField, CThostFtdcQryInvestorPositionField, CThostFtdcReqAuthenticateField, CThostFtdcReqUserLoginField, CThostFtdcRspAuthenticateField, CThostFtdcRspInfoField, CThostFtdcRspUserLoginField, CThostFtdcSettlementInfoConfirmField, THOST_FTDC_CC_Immediately, THOST_FTDC_D_Buy, THOST_FTDC_D_Sell, THOST_FTDC_FCC_NotForceClose, THOST_FTDC_HF_Speculation, THOST_FTDC_OF_Close, THOST_FTDC_OF_Open, THOST_FTDC_OPT_LimitPrice, TraderApi, TraderSpi, TraderSpiInner, TraderSpiStream, THOST_FTDC_TC_GFD, THOST_FTDC_VC_AV, THOST_TE_RESUME_TYPE
    },
};
use polars::prelude::LazyFrame;

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
    stream: TraderSpiStream,
    request_id: i32,
    cash: f32,
    positions: HashMap<String, i32>, // signed position size (+ long / - short)
    broker_id: String,
    investor_id: String,

    pub config: TdAccountConfig,
}

impl LiveBroker {
    fn sync() -> Result<(), String> {
        // Query current positions from broker and update self.positions accordingly.
        // This is a blocking call; should be called infrequently (e.g. once at start).

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
    let resp_stream = Box::new(TraderSpiStream::new(inner));
    tdapi.register_spi(Box::leak(resp_stream) as *mut dyn TraderSpi);

    tdapi.subscribe_private_topic(THOST_TE_RESUME_TYPE::THOST_TERT_QUICK);
    tdapi.subscribe_public_topic(THOST_TE_RESUME_TYPE::THOST_TERT_QUICK);

    tdapi.init();

    while let Some(spi_msg) = resp_stream.next().await {
        match spi_msg {
            OnFrontConnected(p) => {
                info!("前端连接成功回报 OnFrontConnected");
                let mut req = CThostFtdcReqAuthenticateField::default();
                req.BrokerID.assign_from_str(broker_id);
                req.UserID.assign_from_str(account);
                req.AuthCode.assign_from_str(auth_code);
                req.UserProductInfo.assign_from_str(user_product_info);
                req.AppID.assign_from_str(app_id);
                localctp.tdapi.req_authenticate(&mut req, get_request_id());
                info!("call req_authenticate done");
            }
            OnRspAuthenticate(p) => {
                info!("认证成功回报 OnRspAuthenticate");
                // 认证后才能登录
                let mut req = CThostFtdcReqUserLoginField::default();
                req.BrokerID.assign_from_str(broker_id);
                req.UserID.assign_from_str(account);
                req.Password.assign_from_str(&ctp_account.password);
                // 登录后才能下单
                localctp.tdapi.req_user_login(&mut req, get_request_id());
                // 这里有个 break，之后这个 while match 不再接收信息。（推荐将 SPI 放到单独线程）
                break;
            }
            _ => {
                info!("其它回报");
            }
        }
    }

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

pub fn run_td(config: TdAccountConfig, df: LazyFrame) {
    let mut broker = init_api(config);
    let mut strategy = PairStrategy::new_live(
        Params {
            lookback_zscore: 20,
            entry_z: 2.0,
            exit_z: 0.5,
            expiry_close_days: 3,
            debug: true,
            commodity_a_prefix: "ag".into(),
            commodity_b_prefix: "ag".into(),
            transaction_cost_pct: 0.0001,
        },
        df,
    );

    loop {
        println!("td loop");
        thread::sleep(Duration::from_secs(10));

        // Snapshot all current contracts (single lock read) and iterate
        let contracts = snapshot_contracts();
        println!("td snapshot has {} contracts", contracts.len());
        for c in &contracts {
            println!("td sees {} price={} vol={}", c.name, c.price, c.volume);
        }
        let b = contracts.clone(); // intra-commodity pairs for now
        broker.update_positions();
        let _ = strategy.trade(0, contracts, b, &mut broker);
        strategy.pop_spread();
    }

    broker.conclude_equity();
}
