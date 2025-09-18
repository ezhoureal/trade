use futures::StreamExt;
use std::{collections::HashMap, env, thread, time::Duration};

use anyhow::Result;
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
    positions: HashMap<String, i32>, // signed position size (+ long / - short)
    broker_id: String,
    investor_id: String,

    pub config: TdAccountConfig,
}

impl LiveBroker {
    async fn sync(&mut self, contracts: &[ContractData]) -> Result<()> {
        self.request_id += 1;
        let mut qry = CThostFtdcQryTradingAccountField::default();
        qry.InvestorID.assign_from_str(&self.config.td_user_id);
        self.api.req_qry_trading_account(&mut qry, self.request_id);

        for contract in contracts.iter() {
            self.request_id += 1;
            let mut qry = CThostFtdcQryInvestorPositionDetailField::default();
            qry.InvestorID.assign_from_str(&self.config.td_user_id);
            qry.InstrumentID.assign_from_str(contract.name.as_str());
            self.api
                .req_qry_investor_position_detail(&mut qry, self.request_id);
        }

        while let Some(spi_msg) = self.stream.next().await {
            match spi_msg {
                TraderSpiEvent::OnRspQryTradingAccount(event) => {
                    if let Some(acc) = event.trading_account {
                        println!(
                            "OnRspQryTradingAccount: balance={} available={} margin={} ",
                            acc.Balance, acc.Available, acc.CurrMargin
                        );
                        self.cash = acc.Available as f32;
                    }
                }
                TraderSpiEvent::OnRspQryInvestorPositionDetail(event) => {
                    if let Some(pos) = event.investor_position_detail {
                        println!(
                            "OnRspQryInvestorPosition {} pos={}",
                            pos.InstrumentID.to_string(),
                            pos.Volume,
                        );
                        self.positions
                            .insert(pos.InstrumentID.to_string(), pos.Volume);
                    }
                    if event.is_last {
                        break; // all positions received
                    }
                }
                _ => {
                    println!("td sync got event: {:?}", spi_msg);
                }
            }
        }
        Ok(())
    }

    fn order_default(&self, symbol: &str, qty: i32) -> CThostFtdcInputOrderField {
        let mut order = CThostFtdcInputOrderField::default();
        order.BrokerID.assign_from_str(&self.broker_id);
        order.InvestorID.assign_from_str(&self.investor_id);
        order.InstrumentID.assign_from_str(symbol);
        order.Direction = THOST_FTDC_D_Buy as i8;
        order.CombHedgeFlag[0] = THOST_FTDC_HF_Arbitrage as i8;
        order.OrderPriceType = THOST_FTDC_OPT_LimitPrice as i8;
        order.TimeCondition = THOST_FTDC_TC_GFD as i8; // Good For Day
        order.VolumeCondition = THOST_FTDC_VC_MV as i8; // Min volume
        order.MinVolume = 1;
        order.VolumeTotalOriginal = qty.abs();
        order.Direction = if qty > 0 {
            THOST_FTDC_D_Buy as i8
        } else {
            THOST_FTDC_D_Sell as i8
        };
        order
    }

    fn execute_trade(&mut self, symbol: &str, qty: i32) -> Option<i32> {
        let price = get_last_price(symbol)?;
        let mut order = self.order_default(symbol, qty);
        order.LimitPrice = price as f64;

        self.request_id += 1;
        self.api.req_order_insert(&mut order, self.request_id);

        // immediate status update (approximate)
        let size_entry = self.positions.entry(symbol.to_string()).or_insert(0);
        self.cash -= price * qty as f32;
        *size_entry += qty as i32;
        let new_size = *size_entry;
        if new_size == 0 {
            self.positions.remove(symbol);
        }
        Some(new_size)
    }
}

impl Broker for LiveBroker {
    fn exec_open(&mut self, symbol: &str, qty: i32) -> Option<i32> {
        self.execute_trade(symbol, qty)
    }

    fn exec_close(&mut self, symbol: &str, qty: i32) -> Option<i32> {
        self.execute_trade(symbol, qty)
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
                println!("td OnFrontConnected: {:?}", p);
                let mut req = CThostFtdcReqAuthenticateField::default();
                req.UserID.assign_from_str(&config.td_user_id);
                req.AuthCode.assign_from_str(&config.td_auth_code);
                req.AppID.assign_from_str(&config.td_app_id);
                tdapi.req_authenticate(&mut req, 0);
            }
            OnRspAuthenticate(p) => {
                println!("td OnRspAuthenticate: {:?}", p);
                let mut req = CThostFtdcReqUserLoginField::default();
                req.UserID.assign_from_str(&config.td_user_id);
                req.Password.assign_from_str(&config.td_password);
                // // 登录后才能下单
                tdapi.req_user_login(&mut req, 0);
            }
            OnRspUserLogin(p) => {
                println!("td OnRspUserLogin: {:?}", p);
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
        broker_id: env::var("OPENCTP_USER_ID").unwrap_or("".into()),
        investor_id: env::var("OPENCTP_USER_ID").unwrap_or("".into()),
    }
}

#[tokio::main]
pub async fn run_td(config: TdAccountConfig, mut strategy: PairStrategy) -> Result<()> {
    let mut broker = init_api(config).await;
    loop {
        thread::sleep(Duration::from_secs(10));
        // Snapshot all current contracts (single lock read) and iterate
        let contracts = snapshot_contracts();
        println!("td loop, snapshot has {} contracts", contracts.len());
        let b = contracts.clone(); // intra-commodity pairs for now
                                   // broker.update_positions();

        broker.sync(&contracts.clone()).await?;
        strategy.trade(0, contracts, b, &mut broker)?;
        strategy.pop_spread();
    }
}
