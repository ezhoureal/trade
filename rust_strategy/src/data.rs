use anyhow::{Result, anyhow};
use polars::prelude::*;
use chrono::NaiveDate;
use std::path::Path;
use std::fs;

#[derive(Debug, Clone)]
pub struct MarketData {
    pub df: DataFrame,
    pub trading_days: Vec<NaiveDate>,
    pub copper_contracts: Vec<String>,
    pub fuel_contracts: Vec<String>,
}

pub fn load_market_data<P: AsRef<Path>>(path: P) -> Result<MarketData> {
    let path = path.as_ref();
    let mut files: Vec<String> = Vec::new();
    if path.is_file() {
        files.push(path.to_string_lossy().to_string());
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if let Some(ext) = p.extension() { if ext == "parquet" { files.push(p.to_string_lossy().to_string()); }}
        }
    }
    if files.is_empty() { return Err(anyhow!("No parquet files found")); }

    files.sort();
    let mut dfs: Vec<DataFrame> = Vec::new();
    for f in &files {
        let df = ParquetReader::new(std::fs::File::open(f)?).finish()?;
        dfs.push(df);
    }
    // Concatenate DataFrames (concat for LazyFrame expects LazyFrames; use vstack for DataFrames)
    let mut dfs_iter = dfs.into_iter();
    let mut df = dfs_iter.next().ok_or_else(|| anyhow!("No dataframes loaded"))?;
    for d in dfs_iter { df.vstack_mut(&d)?; }

    // Try to locate a date-like column if exact "Date" not present
    if !df.get_column_names().iter().any(|n| n.as_str() == "Date") {
        let maybe_date_alt = df.get_column_names().iter().find(|n| n.as_str().eq_ignore_ascii_case("date")).map(|s| s.to_string());
        if let Some(from_owned) = maybe_date_alt { if from_owned != "Date" { let _ = df.rename(&from_owned, "Date".into()); } }
    }

    // Expect Date either int (yyyymmdd) or string (YYYY-MM-DD / yyyymmdd); normalize to Date column
    if let Ok(col) = df.column("Date") {
        match col.dtype() {
            DataType::String => {
                // Try to parse into Date if not already converted
                let utf = col.str().unwrap();
                let epoch = NaiveDate::from_ymd_opt(1970,1,1).unwrap();
                let days: Vec<Option<i32>> = utf
                    .into_iter()
                    .map(|opt_s| opt_s.and_then(|s| {
                        if s.len()==10 { // maybe YYYY-MM-DD
                            NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
                        } else if s.len()==8 { // yyyymmdd
                            NaiveDate::parse_from_str(s, "%Y%m%d").ok()
                        } else { None }
                            .map(|nd| (nd - epoch).num_days() as i32)
                    }))
                    .collect();
                let date_ca = Int32Chunked::from_iter(days).into_date();
                let mut new_series = date_ca.into_series();
                new_series.rename("Date".into());
                df = df.drop("Date").unwrap_or(df);
                df.with_column(new_series)?;
            }
            DataType::Int32 | DataType::Int64 => {
                // Assume yyyymmdd integer values (e.g., 20210105)
                // Obtain an Int64Chunked safely (avoid referencing temp)
                let date_series: Int64Chunked = if let Ok(ca) = col.i64() {
                    ca.clone()
                } else if let Ok(ca_i32) = col.i32() {
                    ca_i32.cast(&DataType::Int64).unwrap().i64().unwrap().clone()
                } else {
                    return Err(anyhow!("Unsupported Date column type for int parsing"));
                };
                let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
                let days: Vec<Option<i32>> = date_series
                    .into_iter()
                    .map(|opt_v| {
                        opt_v.and_then(|v| {
                            let s = v.to_string();
                            if s.len() == 8 {
                                NaiveDate::parse_from_str(&s, "%Y%m%d").ok().map(|nd| {
                                    (nd - epoch).num_days() as i32
                                })
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
                let date_ca = Int32Chunked::from_iter(days).into_date();
                let mut new_series = date_ca.into_series();
                new_series.rename("Date".into());
                // Replace existing Date column
                if df.get_column_names().iter().any(|n| n.as_str() == "Date") {
                    df = df.drop("Date").unwrap_or(df); // ignore error if drop fails
                }
                df.with_column(new_series)?;    
            },
            DataType::Datetime(_, _) => {
                // Cast to Date (date only)
                if let Ok(casted) = col.cast(&DataType::Date) { let mut s = casted; s.rename("Date".into()); df.with_column(s)?; }
            }
            _ => { /* assume already Date/Utf8 */ }
        }
    }

    // Sort & dedupe
    if df.get_column_names().iter().any(|n| n.as_str() == "Contract") {
        df = df.sort(["Contract", "Date"], SortMultipleOptions::default())?;
        let subset = vec!["Contract".to_string(), "Date".to_string()];
        df = df.unique_stable(Some(&subset), UniqueKeepStrategy::Last, None)?;
    }

    // Collect contracts
    let contract_col = df.column("Contract")?.str()?;
    let mut copper_contracts: Vec<String> = Vec::new();
    let mut fuel_contracts: Vec<String> = Vec::new();
    for v in contract_col.into_iter().flatten() { if v.starts_with("cu") { copper_contracts.push(v.to_string()) } else if v.starts_with("fu") { fuel_contracts.push(v.to_string()) } }

    // Extract trading days
    let date_col = df.column("Date")?;
    let mut trading_days: Vec<NaiveDate> = Vec::new();
    match date_col.dtype() { 
        DataType::Date => {
            let date_chunked = date_col.date()?;
            for o in date_chunked.into_iter() { if let Some(days) = o { // polars Date = days since 1970-01-01
                let nd = NaiveDate::from_ymd_opt(1970,1,1).unwrap().checked_add_signed(chrono::Duration::days(days as i64)).unwrap();
                trading_days.push(nd);
            }}
        }
        _ => {
            // Fallback: try interpret as String
            if let Ok(utf) = date_col.str() {
                for s_opt in utf.into_iter() { if let Some(s) = s_opt {
                    let parsed = if s.len()==10 { NaiveDate::parse_from_str(s, "%Y-%m-%d").ok() } else if s.len()==8 { NaiveDate::parse_from_str(s, "%Y%m%d").ok() } else { None };
                    if let Some(nd) = parsed { trading_days.push(nd); }
                }}
            }
        }
    }

    trading_days.sort();
    trading_days.dedup();

    if trading_days.is_empty() {
        eprintln!("Warning: no trading days parsed; check 'Date' column format. Columns present: {:?}", df.get_column_names());
        if let Ok(series) = df.column("Date") { eprintln!("Date column dtype: {:?}, first 5 values: {:?}", series.dtype(), series.head(Some(5))); }
    }
    Ok(MarketData { df, trading_days, copper_contracts, fuel_contracts })
}
