use std::sync::Arc;

use ctp2rs::{
    ffi::{gb18030_cstr_i8_to_str, AssignFromString, WrapToString},
    print_rsp_info,
    v1alpha1::{
        CThostFtdcDepthMarketDataField, CThostFtdcReqUserLoginField, CThostFtdcRspInfoField,
        CThostFtdcRspUserLoginField, MdApi, MdSpi,
    },
};

use crate::market_data::update_quote;
use crate::MdAccountConfig;

pub struct BaseMdSpi {
    pub(crate) md_api: Arc<MdApi>,
    pub(crate) config: MdAccountConfig,
    contracts: Vec<String>,
}

impl MdSpi for BaseMdSpi {
    fn on_front_connected(&mut self) {
        // println!("[MD_SPI] on_front_connected - Connected to market data server");
        let mut req = CThostFtdcReqUserLoginField::default();
        req.BrokerID.assign_from_str("9999");
        req.UserID.assign_from_str("");
        println!("[MD_SPI] Sending login request - BrokerID: 9999, UserID: {}", self.config.md_user_id);
        self.md_api.req_user_login(&mut req, 1);
        self.md_api.subscribe_market_data(&self.contracts);
    }

    fn on_rsp_user_login(
        &mut self,
        rsp_user_login: Option<&CThostFtdcRspUserLoginField>,
        rsp_info: Option<&CThostFtdcRspInfoField>,
        request_id: i32,
        is_last: bool,
    ) {
        println!("[MD_SPI] on_rsp_user_login - request_id: {}, is_last: {}", request_id, is_last);

        if let Some(info) = rsp_info {
            if info.ErrorID != 0 {
                println!("[MD_SPI] Login FAILED - Error ID: {}", info.ErrorID);
                println!("[MD_SPI] Login FAILED - Error Message: {}", info.ErrorMsg.to_string());
                return;
            }
        }

        if let Some(login) = rsp_user_login {
            println!("[MD_SPI] Login SUCCESS - TradingDay: {}", login.TradingDay.to_string());
            println!("[MD_SPI] Login SUCCESS - LoginTime: {}", login.LoginTime.to_string());
        }

        print_rsp_info!(rsp_info);
        println!(
            "[MD_SPI] Subscribing to {} contracts: {:?}",
            self.contracts.len(),
            self.contracts
        );

        if is_last {
            self.md_api.subscribe_market_data(&self.contracts);
            println!("[MD_SPI] Market data subscription request sent");
        }
    }

    fn on_rtn_depth_market_data(
        &mut self,
        depth_market_data: Option<&CThostFtdcDepthMarketDataField>,
    ) {
        if let Some(q) = depth_market_data {
            let instrument = q.InstrumentID.to_string();
            update_quote(&instrument, q.LastPrice as f32, q.Volume as u32);
            // Uncomment for verbose market data logging:
            println!(
                "[MD_SPI] Market data: {} lastPrice = {}, volume={}",
                instrument, q.LastPrice, q.Volume
            );
        }
    }

    fn on_rsp_error(&mut self, p_rsp_info: Option<&CThostFtdcRspInfoField>, n_request_id: i32, b_is_last: bool) {
        println!("[MD_SPI] on_rsp_error - request_id: {}, is_last: {}", n_request_id, b_is_last);
        if let Some(info) = p_rsp_info {
            println!("[MD_SPI] Error ID: {}", info.ErrorID);
            println!("[MD_SPI] Error Message: {}", info.ErrorMsg.to_string());
        } else {
            println!("[MD_SPI] No error info provided");
        }
        print_rsp_info!(p_rsp_info);
    }
}

pub fn run_md(config: MdAccountConfig, contracts: Vec<String>) {
    println!(
        "md dynlib_path: {}",
        config.md_dynlib_path.to_string_lossy()
    );

    println!("[MD] Creating MdApi instance");
    let mdapi = MdApi::create_api(&config.md_dynlib_path, "./md_", false, false, true);

    let md_api = Arc::new(mdapi);
    println!("md get_api_version: {}", md_api.get_api_version());

    let front_address = config.md_front_address.clone();

    md_api.register_front(&front_address);

    // Leak the Box; the pointer now has 'static lifetime (never freed until process exit).
    let md_spi_box = Box::new(BaseMdSpi {
        md_api: Arc::clone(&md_api),
        config,
        contracts,
    });
    let spi: *mut BaseMdSpi = Box::leak(md_spi_box);
    md_api.register_spi(spi as *mut dyn MdSpi);

    md_api.init();
    println!("mdapi init");
}
