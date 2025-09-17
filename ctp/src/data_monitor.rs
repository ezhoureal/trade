use std::sync::Arc;

use ctp2rs::{
    ffi::{gb18030_cstr_i8_to_str, AssignFromString, WrapToString},
    print_rsp_info,
    v1alpha1::{
        CThostFtdcDepthMarketDataField, CThostFtdcReqUserLoginField, CThostFtdcRspInfoField,
        CThostFtdcRspUserLoginField, CThostFtdcSpecificInstrumentField, MdApi, MdSpi,
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
        let mut req = CThostFtdcReqUserLoginField::default();
        println!("mdspi.on_front_connected");
        req.UserID.assign_from_str(&self.config.md_user_id);
        self.md_api.req_user_login(&mut req, 1);
    }

    fn on_rsp_user_login(
        &mut self,
        _rsp_user_login: Option<&CThostFtdcRspUserLoginField>,
        rsp_info: Option<&CThostFtdcRspInfoField>,
        _request_id: i32,
        is_last: bool,
    ) {
        print_rsp_info!(rsp_info);
        println!(
            "on_rsp_user_login! subscribing to contracts {:?}",
            self.contracts
        );

        if is_last {
            self.md_api.subscribe_market_data(&self.contracts);
        }
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
            println!(
                "md update: {} -> {}",
                instrument.to_ascii_lowercase(),
                last_price
            );
        }
    }
}

pub fn run_md(config: MdAccountConfig, contracts: Vec<String>) {
    println!(
        "md dynlib_path: {}",
        config.md_dynlib_path.to_string_lossy()
    );

    #[cfg(not(feature = "ctp_v6_7_11"))]
    let mdapi = MdApi::create_api(&config.md_dynlib_path, "./md_", false, false);

    #[cfg(feature = "ctp_v6_7_11")]
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
