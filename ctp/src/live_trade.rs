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
        CThostFtdcInputOrderField,
        CThostFtdcInvestorPositionField,
        CThostFtdcQryInvestorPositionField,
        CThostFtdcReqAuthenticateField,
        CThostFtdcReqUserLoginField,
        CThostFtdcRspAuthenticateField,
        CThostFtdcRspInfoField,
        CThostFtdcRspUserLoginField,
        CThostFtdcSettlementInfoConfirmField,
        THOST_FTDC_CC_Immediately,
        // constants
        THOST_FTDC_D_Buy,
        THOST_FTDC_D_Sell,
        THOST_FTDC_FCC_NotForceClose,
        THOST_FTDC_HF_Speculation,
        THOST_FTDC_OF_Close,
        THOST_FTDC_OF_Open,
        THOST_FTDC_OPT_LimitPrice,
        TraderApi,
        TraderSpi,
        THOST_FTDC_TC_GFD,
        THOST_FTDC_VC_AV,
        THOST_TE_RESUME_TYPE,
    },
};

use crate::{
    market_data::{get_last_price, snapshot_contracts},
    TdAccountConfig,
};

pub struct BaseTraderSpi {
    pub tdapi: Arc<TraderApi>,
    pub request_id: i32,
    pub config: TdAccountConfig,
}

impl TraderSpi for BaseTraderSpi {
    fn on_front_connected(&mut self) {
        println!("tdspi.on_front_connected !!!");
        let mut req = CThostFtdcReqAuthenticateField::default();
        req.BrokerID.assign_from_str("9999");
        req.UserID.assign_from_str(&self.config.td_user_id);
        req.AppID.assign_from_str(&self.config.td_app_id);
        req.AuthCode.assign_from_str(&self.config.td_auth_code);

        self.request_id += 1;
        self.tdapi.req_authenticate(&mut req, self.request_id);
    }

    fn on_front_disconnected(&mut self, n_reason: i32) {
        println!("on_front_disconnected: reason -> {n_reason}")
    }

    fn on_heart_beat_warning(&mut self, n_time_lapse: i32) {}

    fn on_rsp_authenticate(
        &mut self,
        p_rsp_authenticate_field: Option<&CThostFtdcRspAuthenticateField>,
        p_rsp_info: Option<&CThostFtdcRspInfoField>,
        n_request_id: i32,
        b_is_last: bool,
    ) {
        println!("on_rsp_authenticate");
        print_rsp_info!(p_rsp_info);
        if let Some(p_rsp_info) = p_rsp_info {
            if p_rsp_info.ErrorID != 0 {
                return;
            }
        }

        if b_is_last {
            let mut req = CThostFtdcReqUserLoginField::default();
            req.BrokerID.assign_from_str("9999");
            req.UserID.assign_from_str(&self.config.td_user_id);
            req.Password.assign_from_str(&self.config.td_password);

            self.request_id += 1;
            let ret = self.tdapi.req_user_login(&mut req, self.request_id);
            println!("req_user_login result: {ret}");
        }
    }

    fn on_rsp_user_login(
        &mut self,
        p_rsp_user_login: Option<&CThostFtdcRspUserLoginField>,
        p_rsp_info: Option<&CThostFtdcRspInfoField>,
        n_request_id: i32,
        b_is_last: bool,
    ) {
        print_rsp_info!(p_rsp_info);
        if b_is_last {
            let mut req = CThostFtdcSettlementInfoConfirmField::default();
            req.BrokerID.assign_from_str("9999");
            req.InvestorID.assign_from_str(&self.config.td_user_id);

            self.request_id += 1;
            let ret = self
                .tdapi
                .req_settlement_info_confirm(&mut req, self.request_id);
            println!("req_user_login result: {ret}");
        }
    }

    fn on_rsp_settlement_info_confirm(
        &mut self,
        p_settlement_info_confirm: Option<&CThostFtdcSettlementInfoConfirmField>,
        p_rsp_info: Option<&CThostFtdcRspInfoField>,
        n_request_id: i32,
        b_is_last: bool,
    ) {
        print_rsp_info!(p_rsp_info);
        if b_is_last {
            std::thread::sleep(std::time::Duration::from_secs(1));
            self.request_id += 1;
            let mut req = CThostFtdcQryInvestorPositionField::default();
            let ret = self
                .tdapi
                .req_qry_investor_position(&mut req, self.request_id);
            println!("req_qry_investor_position result: {ret}");
        }
    }

    fn on_rsp_qry_investor_position(
        &mut self,
        p_investor_position: Option<&CThostFtdcInvestorPositionField>,
        p_rsp_info: Option<&CThostFtdcRspInfoField>,
        n_request_id: i32,
        b_is_last: bool,
    ) {
        print_rsp_info!(p_rsp_info);
        if let Some(p) = p_investor_position {
            let instrument_id = p.InstrumentID.to_string();
            let user_id = p.InvestorID.to_string();
            println!("{user_id} holds {instrument_id}");
        } else {
            println!("position hold None");
        }
        if b_is_last {
            println!("on_rsp_qry_investor_position finish!");
        }
    }
}

/// Very lightweight live broker implementation.
/// Currently this manages a virtual position & cash book locally while
/// delegating eventual real order routing integration to future work.
/// Once order callbacks are wired, the req_order_insert calls (commented)
/// can replace the virtual fills below.
struct LiveBroker {
    api: Arc<TraderApi>,
    request_id: i32,
    cash: f32,
    positions: HashMap<String, i32>, // signed position size (+ long / - short)
    broker_id: String,
    investor_id: String,
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

fn init_api(config: TdAccountConfig) -> Arc<TraderApi> {
    println!("tdapi start here!");
    println!(
        "td dynlib_path: {}",
        config.td_dynlib_path.to_string_lossy()
    );

    #[cfg(not(feature = "ctp_v6_7_11"))]
    let tdapi = TraderApi::create_api(&config.td_dynlib_path, "./td_");

    #[cfg(feature = "ctp_v6_7_11")]
    let tdapi = TraderApi::create_api(&config.td_dynlib_path, "./td_", true);

    let tdapi = Arc::new(tdapi);

    let front_address = config.td_front_address.clone();
    println!("td get_api_version: {}", tdapi.get_api_version());

    tdapi.register_front(&front_address);

    let tdspi_box = Box::new(BaseTraderSpi {
        tdapi: Arc::clone(&tdapi),
        request_id: 0,
        config,
    });
    tdapi.register_spi(Box::leak(tdspi_box));

    tdapi.subscribe_private_topic(THOST_TE_RESUME_TYPE::THOST_TERT_QUICK);
    tdapi.subscribe_public_topic(THOST_TE_RESUME_TYPE::THOST_TERT_QUICK);

    tdapi.init();

    println!("tdapi init");
    tdapi
}

pub fn run_td(config: TdAccountConfig) {
    let tdapi = init_api(config);
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
        "../data/recent.parquet".into(),
    );
    const STARTING_CASH: f32 = 1_000_000.0;
    // Capture IDs before config is moved into init_api
    // We don't retain password/auth code here for safety.
    let investor_id = "todo_set_user_id".to_string(); // TODO: pass through actual config user id
    let broker_id = "9999".to_string();
    let mut broker = LiveBroker {
        api: tdapi,
        request_id: 0,
        cash: STARTING_CASH,
        positions: HashMap::new(),
        broker_id,
        investor_id,
    };

    loop {
        println!("td loop");
        thread::sleep(Duration::from_secs(10));

        // Snapshot all current contracts (single lock read) and iterate
        let contracts = snapshot_contracts();
        println!("td snapshot has {} contracts", contracts.len());
        for c in &contracts {
            println!("td sees {} price={} vol={}", c.name, c.price, c.volume);
        }
        let _ = strategy.trade(0, contracts.clone(), contracts, &mut broker);
    }
}
