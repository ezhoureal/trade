#![allow(unused_variables)]
use std::{sync::{Arc, Mutex}, thread, time::Duration};

use backtest::strategy::PairStrategy;
use ctp2rs::{
    ffi::{gb18030_cstr_i8_to_str, AssignFromString, WrapToString},
    print_rsp_info,
    v1alpha1::{
        CThostFtdcDepthMarketDataField, CThostFtdcReqUserLoginField, CThostFtdcRspInfoField,
        CThostFtdcRspUserLoginField, CThostFtdcSpecificInstrumentField, MdApi, MdSpi,
    },
};

use crate::CtpAccountConfig;
use crate::market_data::update_quote;

pub struct BaseMdSpi {
    pub(crate) mdapi: Arc<MdApi>,
    pub(crate) config: CtpAccountConfig,
}

impl MdSpi for BaseMdSpi {
    fn on_front_connected(&mut self) {
        let mut req = CThostFtdcReqUserLoginField::default();
        println!("mdspi.on_front_connected");
        req.UserID.assign_from_str(&self.config.md_user_id);
        self.mdapi.req_user_login(&mut req, 1);
    }

    fn on_rsp_user_login(
        &mut self,
        rsp_user_login: Option<&CThostFtdcRspUserLoginField>,
        rsp_info: Option<&CThostFtdcRspInfoField>,
        request_id: i32,
        is_last: bool,
    ) {
        print_rsp_info!(rsp_info);
        println!("on_rsp_user_login!");

        // if is_last {
        //     let instrument_ids = vec!["ag2512".to_string(), "au2512".to_string()];
        //     self.mdapi.subscribe_market_data(&instrument_ids);
        // }
    }

    fn on_rsp_sub_market_data(
        &mut self,
        specific_instrument: Option<&CThostFtdcSpecificInstrumentField>,
        rsp_info: Option<&CThostFtdcRspInfoField>,
        request_id: i32,
        is_last: bool,
    ) {
        print_rsp_info!(rsp_info);
        println!(
            "on_rsp_sub_market_data: instrument_id[{:?}], {:?}, {:?}",
            specific_instrument.unwrap().InstrumentID.to_string(),
            request_id,
            is_last
        );
    }

    fn on_rtn_depth_market_data(
        &mut self,
        depth_market_data: Option<&CThostFtdcDepthMarketDataField>,
    ) {
        println!("OnRtnDepthMarketData!");

        if let Some(q) = depth_market_data {
            let instrument = q.InstrumentID.to_string();
            let last_price = q.LastPrice as f32;
            // For now we don't have volume in this callback struct? If available use q.Volume (example) else 0
            let volume: u32 = 0; // placeholder until correct field identified
            update_quote(&instrument, last_price, volume);
            println!("md update: {} -> {}", instrument.to_ascii_lowercase(), last_price);
        }
    }
}

pub fn run_md(config: CtpAccountConfig, strategy_ctx: Arc<Mutex<PairStrategy>>) {
    println!("mdapi start here!");
    println!(
        "md dynlib_path: {}",
        config.md_dynlib_path.to_string_lossy()
    );

    #[cfg(not(feature = "ctp_v6_7_11"))]
    let mdapi = MdApi::create_api(&config.md_dynlib_path, "./md_", false, false);

    #[cfg(feature = "ctp_v6_7_11")]
    let mdapi = MdApi::create_api(&config.md_dynlib_path, "./md_", false, false, true);

    let mdapi = Arc::new(mdapi);

    // 先获取 front_address，避免 move 后的借用问题
    let front_address = config.md_front_address.clone();

    let base_mdspi: BaseMdSpi = BaseMdSpi { mdapi: Arc::clone(&mdapi), config };
    let mdspi_box = Box::new(base_mdspi);
    println!("md get_api_version: {}", mdapi.get_api_version());

    mdapi.register_front(&front_address);

    let mdspi_ptr = Box::into_raw(mdspi_box) as *mut dyn MdSpi;
    let mdspi_ptr2 = mdspi_ptr.clone();

    mdapi.register_spi(mdspi_ptr);

    mdapi.init();
    let instruments = vec!["ag2512".to_string(), "au2601".to_string()];
    mdapi.subscribe_market_data(&instruments);

    println!("mdapi init");

    loop {
        println!("md loop");
        thread::sleep(Duration::from_secs(10));
    }
}