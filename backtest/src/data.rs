use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use polars::prelude::*;
use std::fs;
use std::path::Path;

pub fn load_market_data<P: AsRef<Path>>(path: P) -> Result<DataFrame> {
    let path = path.as_ref();
    let mut files: Vec<String> = Vec::new();
    if path.is_file() {
        files.push(path.to_string_lossy().to_string());
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if let Some(ext) = p.extension() {
                if ext == "parquet" {
                    files.push(p.to_string_lossy().to_string());
                }
            }
        }
    }
    if files.is_empty() {
        return Err(anyhow!("No parquet files found"));
    }

    files.sort();
    let lazy_frames: Vec<LazyFrame> = files
        .iter()
        .map(|f| LazyFrame::scan_parquet(f.as_str(), ScanArgsParquet::default()))
        .collect::<PolarsResult<Vec<_>>>()?;

    let acc = concat(lazy_frames, UnionArgs::default())?;
    Ok(acc.collect()?)
}

pub fn filter_contract_by_prefix(df: DataFrame, prefix_a: &str, prefix_b: &str) -> Result<DataFrame> {
    let contract_col = df.column("Contract")?.str()?;
    let bools: BooleanChunked = contract_col
        .into_iter()
        .map(|opt| opt.map(|s| s.starts_with(prefix_a) || s.starts_with(prefix_b)))
        .collect();
    Ok(df.filter(&bools)?)
}

pub fn normalize_date_to_bar(mut df: DataFrame) -> Result<DataFrame> {
    let col_name = "Date";
    let date_series = df.column(col_name)?;
    let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();

    // Coerce to days since epoch in Int32
    let days_i32: Int32Chunked = match date_series.dtype() {
        DataType::Date => date_series.clone().cast(&DataType::Int32)?.i32()?.clone(),
        DataType::Datetime(_, _) => date_series
            .clone()
            .cast(&DataType::Date)?
            .cast(&DataType::Int32)?
            .i32()?
            .clone(),
        DataType::String => {
            let parsed: Vec<Option<i32>> = date_series
                .str()?
                .into_iter()
                .map(|opt_s| {
                    opt_s.map(|s| {
                        let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")
                            .expect("Failed to parse date string");
                        (d - epoch).num_days() as i32
                    })
                })
                .collect();
            Int32Chunked::from_iter(parsed)
        }
        DataType::Int32 => date_series.i32()?.clone(),
        DataType::Int64 => {
            let mut out: Vec<Option<i32>> = Vec::with_capacity(date_series.len());
            for v in date_series.i64()?.into_iter() {
                out.push(v.map(|x| x as i32));
            }
            Int32Chunked::from_iter(out)
        }
        other => return Err(anyhow!("Unsupported date column dtype: {:?}", other)),
    };

    // Mapping via unique + sort + join
    let mut unique_days = days_i32.clone().into_series();
    unique_days = unique_days.unique()?; // remove duplicates (order unspecified)
    let sort_opts = SortOptions {
        descending: false,
        nulls_last: false,
        multithreaded: true,
        maintain_order: false,
        limit: None,
    };
    unique_days = unique_days.sort(sort_opts)?; // ascending
    unique_days.rename("__day_key".into());

    let bar_vec: Vec<u32> = (0..unique_days.len() as u32).collect();
    let bar_series_map = Series::new("Bar".into(), bar_vec);
    let map_df = DataFrame::new(vec![unique_days.clone().into(), bar_series_map.into()])?;

    let mut orig_days = days_i32.into_series();
    orig_days.rename("__day_key".into());
    let orig_df = DataFrame::new(vec![orig_days.into()])?;

    let join_args = JoinArgs::new(JoinType::Left);
    let joined = orig_df.join(&map_df, ["__day_key"], ["__day_key"], join_args, None)?;
    let mut bar_series = joined.column("Bar")?.clone();
    bar_series.rename("Bar".into());

    if df.get_column_index("Bar").is_some() { df.drop_in_place("Bar")?; }
    df.with_column(bar_series)?;
    Ok(df)
}

#[test]
fn test_filter_contract_by_prefix() {
    // Create sample data with various contract names
    let contracts = Series::new(
        "Contract".into(),
        &[
            "ES_202112",
            "NQ_202112",
            "CL_202112",
            "GC_202112",
            "ES_202203",
            "ZN_202112",
            "NQ_202203",
        ],
    );
    let prices = Series::new(
        "Close".into(),
        &[4500.0f64, 15000.0, 80.0, 1800.0, 4600.0, 130.0, 15200.0],
    );
    let df = DataFrame::new(vec![contracts.into(), prices.into()]).unwrap();

    // Filter for ES and NQ contracts
    let df = filter_contract_by_prefix(df, "ES_", "NQ_").unwrap();

    let contract_series = df.column("Contract").unwrap();
    let filtered_contracts: Vec<&str> = contract_series
        .str()
        .unwrap()
        .into_iter()
        .map(|o| o.unwrap())
        .collect();

    // Should only contain ES and NQ contracts
    assert_eq!(
        filtered_contracts,
        vec!["ES_202112", "NQ_202112", "ES_202203", "NQ_202203"]
    );
    assert_eq!(df.height(), 4);
}

#[test]
fn test_filter_contract_by_prefix_empty_result() {
    let contracts = Series::new("Contract".into(), &["CL_202112", "GC_202112", "ZN_202112"]);
    let prices = Series::new("Close".into(), &[80.0f64, 1800.0, 130.0]);
    let df = DataFrame::new(vec![contracts.into(), prices.into()]).unwrap();

    // Filter for prefixes that don't exist
    let df = filter_contract_by_prefix(df, "ES_", "NQ_").unwrap();
    assert_eq!(df.height(), 0);
}

#[test]
fn test_filter_contract_by_prefix_single_match() {
    let contracts = Series::new("Contract".into(), &["ES_202112", "CL_202112", "GC_202112"]);
    let prices = Series::new("Close".into(), &[4500.0f64, 80.0, 1800.0]);
    let df = DataFrame::new(vec![contracts.into(), prices.into()]).unwrap();

    // Filter for only ES contracts
    let df = filter_contract_by_prefix(df, "ES_", "XX_").unwrap();

    let contract_series = df.column("Contract").unwrap();
    let filtered_contracts: Vec<&str> = contract_series
        .str()
        .unwrap()
        .into_iter()
        .map(|o| o.unwrap())
        .collect();

    assert_eq!(filtered_contracts, vec!["ES_202112"]);
    assert_eq!(df.height(), 1);
}

#[test]
fn test_date_remap_simple() {
    // Create sample dates (unordered with duplicates)
    let dates = Series::new(
        "Date".into(),
        &[
            "2021-01-05",
            "2021-01-04",
            "2021-01-05",
            "2021-01-06",
            "2021-01-04",
        ],
    );
    let prices = Series::new("Close".into(), &[1.0f64, 2.0, 3.0, 4.0, 5.0]);
    let df = DataFrame::new(vec![dates.into(), prices.into()]).unwrap();
    let df2 = normalize_date_to_bar(df).unwrap();
    let date_series = df2.column("Bar").unwrap();
    assert_eq!(date_series.dtype(), &DataType::UInt32);
    let vals: Vec<u32> = date_series
        .u32()
        .unwrap()
        .into_iter()
        .map(|o| o.unwrap())
        .collect();
    // Unique original days sorted: 2021-01-04, 2021-01-05, 2021-01-06 -> indices 0,1,2
    // Original rows mapping: [2021-01-05(1), 2021-01-04(0), 2021-01-05(1), 2021-01-06(2), 2021-01-04(0)]
    assert_eq!(vals, vec![1, 0, 1, 2, 0]);
}
