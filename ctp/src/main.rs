use backtest::{params::{Commodity, Params}, strategy::PairStrategy};
use clap::{Parser, ValueEnum};
use polars::prelude::*;
use std::{path::PathBuf, thread};

mod data_monitor;
mod live_trade;
mod market_data;

use data_monitor::*;
use live_trade::*;

#[derive(Debug, Clone, ValueEnum)]
pub enum Environment {
    /// 仿真环境
    Sim,
    /// 7x24小时模拟环境
    Tts,
}

#[derive(Debug, Clone)]
pub struct MdAccountConfig {
    // mdapi 配置
    pub md_user_id: String,
    pub md_front_address: String,
    pub md_dynlib_path: PathBuf,
}
pub struct TdAccountConfig {
    // tdapi 配置
    pub td_user_id: String,
    pub td_password: String,
    pub td_app_id: String,
    pub td_auth_code: String,
    pub td_front_address: String,
    pub td_dynlib_path: PathBuf,
    pub special_close_all: bool
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 选择运行环境
    #[arg(short, long, value_enum, default_value_t = Environment::Tts)]
    environment: Environment,

    /// 用户ID
    #[arg(short, long, env = "OPENCTP_USER_ID")]
    user_id: String,

    /// 密码
    #[arg(short, long, env = "OPENCTP_PASS")]
    password: String,

    /// 特殊平仓模式
    #[arg(long = "special-close-all")]
    close_all: bool,
}

fn create_config_for_environment(
    args: Args,
) -> (MdAccountConfig, TdAccountConfig) {
    let base_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let base_path = std::path::Path::new(&base_dir);

    match args.environment {
        Environment::Sim => {
            // 仿真环境配置
            #[cfg(target_os = "macos")]
            let md_dynlib_path = base_path.join("api/v6.7.2_MacOS_20231016/thostmduserapi_se.framework/thostmduserapi_se");
            #[cfg(all(target_os = "linux", not(feature = "ctp_v6_7_11")))]
            let md_dynlib_path = base_path.join("../../../ctp-dyn/api/ctp/v6.7.2/v6.7.2_20230913_api_traderapi_se_linux64/thostmduserapi_se.so");
            #[cfg(all(target_os = "linux", feature = "ctp_v6_7_11"))]
            let md_dynlib_path = base_path.join("../../../ctp-dyn/api/ctp/v6.7.11/v6.7.11_20250617_api_traderapi_se_linux64/thostmduserapi_se.so");

            #[cfg(all(target_os = "windows", not(feature = "ctp_v6_7_11")))]
            let md_dynlib_path = base_path.join("../../../ctp-dyn/api/ctp/v6.7.2/v6.7.2_20230913_api_traderapi_se_win64/thostmduserapi_se.dll");
            #[cfg(all(target_os = "windows", feature = "ctp_v6_7_11"))]
            let md_dynlib_path = base_path.join("../../../ctp-dyn/api/ctp/v6.7.11/v6.7.11_20250617_traderapi64_se_windows/thostmduserapi_se.dll");

            #[cfg(target_os = "macos")]
            let td_dynlib_path = base_path.join("api/mac_arm64/thosttraderapi_se.dylib");
            #[cfg(target_os = "linux")]
            let td_dynlib_path = base_path.join("api/lin64/thosttraderapi_se.so");
            #[cfg(target_os = "windows")]
            let td_dynlib_path = base_path.join("api/win64/thosttraderapi_se.dll");

            (
                MdAccountConfig {
                    md_user_id: "247486".to_string(),
                    md_front_address: "tcp://182.254.243.31:30011".to_string(), // SimNow 仿真环境
                    md_dynlib_path,
                },
                TdAccountConfig {
                    td_user_id: "14572".to_string(),
                    td_password: args.password,
                    td_app_id: "simnow_client_test".to_string(),
                    td_auth_code: "0000000000000000".to_string(),
                    td_front_address: "tcp://121.37.90.193:20002".to_string(), // OPENCTP 仿真环境
                    td_dynlib_path,
                    special_close_all: args.close_all
                },
            )
        }
        Environment::Tts => {
            // 7x24小时模拟环境配置
            #[cfg(target_os = "macos")]
            let md_dynlib_path = base_path.join("api/mac_arm64/thostmduserapi_se.dylib");
            #[cfg(target_os = "linux")]
            let md_dynlib_path = base_path.join("api/lin64/thostmduserapi_se.so");
            #[cfg(target_os = "windows")]
            let md_dynlib_path = base_path.join("api/win64/thostmduserapi_se.dll");

            #[cfg(target_os = "macos")]
            let td_dynlib_path = base_path.join("api/mac_arm64/thosttraderapi_se.dylib");
            #[cfg(target_os = "linux")]
            let td_dynlib_path = base_path.join("api/lin64/thosttraderapi_se.so");
            #[cfg(target_os = "windows")]
            let td_dynlib_path = base_path.join("api/win64/thosttraderapi_se.dll");

            (
                MdAccountConfig {
                    md_user_id: args.user_id.clone(),
                    md_front_address: "tcp://121.37.80.177:20004".to_string(), // TTS 7x24 环境
                    md_dynlib_path,
                },
                TdAccountConfig {
                    td_user_id: args.user_id,
                    td_password: args.password,
                    td_app_id: "simnow_client_test".to_string(),
                    td_auth_code: "0000000000000000".to_string(),
                    td_front_address: "tcp://121.37.80.177:20002".to_string(), // TTS 7x24 环境
                    td_dynlib_path,
                    special_close_all: args.close_all,
                },
            )
        }
    }
}

fn main() -> PolarsResult<()> {
    let args = Args::parse();

    println!("Running with environment: {:?}", args.environment);

    let config = create_config_for_environment(args);

    let df = LazyFrame::scan_parquet("../data/recent.parquet", ScanArgsParquet::default())?;
    let contracts_df = df
        .clone()
        .unique(
            Some(vec!["Contract".to_string()]),
            UniqueKeepStrategy::First,
        )
        .collect()?;
    let contracts: Vec<String> = contracts_df
        .column("Contract")?
        .str()?
        .into_iter()
        .filter_map(|opt| opt.map(|s| s.to_string()))
        .collect();

    let md_thread = thread::spawn(move || {
        run_md(config.0, contracts);
    });

    let mut strategy = PairStrategy::new_live(
        Params {
            lookback_zscore: 20,
            entry_z: 2.0,
            exit_z: 0.5,
            expiry_close_days: 3,
            debug: true,
            a: Commodity::default(),
            b: Commodity::default(),
            hedge_ratio: 1.0,
        },
        df,
    );
    run_td(config.1, &mut strategy).unwrap();
    md_thread.join().unwrap();
    Ok(())
}
