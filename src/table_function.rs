use aws_config::BehaviorVersion;
use aws_sdk_athena::{
    operation::get_query_execution::GetQueryExecutionOutput,
    operation::get_query_results::GetQueryResultsOutput,
    types::{
        QueryExecutionState::{self, *},
        ResultConfiguration, ResultSetMetadata, Row,
    },
    Client as AthenaClient,
};
use aws_sdk_glue::Client as GlueClient;
use libduckdb_sys::{duckdb_data_chunk, duckdb_data_chunk_set_size, duckdb_bind_info, duckdb_function_info, duckdb_function_set_error, duckdb_init_info, idx_t};
use quack_rs::{
    table::{BindInfo, FfiBindData, FfiInitData, TableFunctionBuilder},
    types::TypeId,
};
use std::{ffi::CString, thread};
use tokio::time::Duration;

use crate::types::{map_type, populate_column};

const DEFAULT_LIMIT: i32 = 10000;

struct ScanBindData {
    tablename: String,
    database: String,
    output_location: String,
    limit: i32,
}

impl ScanBindData {
    fn new(tablename: &str, database: &str, output_location: &str, limit: i32) -> Self {
        Self {
            tablename: tablename.to_owned(),
            database: database.to_owned(),
            output_location: output_location.to_owned(),
            limit,
        }
    }
}

struct ScanInitData {
    pages: Vec<GetQueryResultsOutput>,
    current_page: usize,
    done: bool,
}

impl ScanInitData {
    fn new(pages: Vec<GetQueryResultsOutput>) -> Self {
        Self {
            pages,
            current_page: 0,
            done: false,
        }
    }
}

/// # Safety
#[no_mangle]
unsafe extern "C" fn read_athena(info: duckdb_function_info, output: duckdb_data_chunk) {
    unsafe {
        let init_data = FfiInitData::<ScanInitData>::get_mut(info);
        if let Some(state) = init_data {
            if state.done || state.current_page >= state.pages.len() {
                duckdb_data_chunk_set_size(output, 0);
                state.done = true;
                return;
            }

            let page = &state.pages[state.current_page];
            if let Some(rs) = page.result_set() {
                let rows = rs.rows();
                // Athena returns the column header in the first page's first row
                let rows_slice: &[Row] = if state.current_page == 0 && !rows.is_empty() {
                    &rows[1..]
                } else {
                    rows
                };

                if let Some(metadata) = rs.result_set_metadata() {
                    if let Err(e) = result_set_to_duckdb_data_chunk(rows_slice, metadata, output) {
                        let msg = CString::new(e.to_string()).unwrap_or_default();
                        duckdb_function_set_error(info, msg.as_ptr());
                        duckdb_data_chunk_set_size(output, 0);
                        state.done = true;
                        return;
                    }
                } else {
                    duckdb_data_chunk_set_size(output, 0);
                    state.done = true;
                }
            } else {
                duckdb_data_chunk_set_size(output, 0);
                state.done = true;
            }
            state.current_page += 1;
        } else {
            duckdb_data_chunk_set_size(output, 0);
        }
    }
}

pub fn result_set_to_duckdb_data_chunk(
    rows: &[Row],
    metadata: &ResultSetMetadata,
    chunk: duckdb_data_chunk,
) -> anyhow::Result<()> {
    let result_size = rows.len();
    let col_infos = metadata.column_info();
    let chunk_col_count = unsafe { libduckdb_sys::duckdb_data_chunk_get_column_count(chunk) } as usize;

    for row_idx in 0..result_size {
        let row = &rows[row_idx];
        let row_data = row.data();
        for col_idx in 0..row_data.len() {
            // Guard against both Athena metadata and DuckDB chunk column counts.
            // They should be equal, but if they diverge (e.g. unsupported column
            // types were skipped) we must not write past the chunk boundary.
            if col_idx >= col_infos.len() || col_idx >= chunk_col_count {
                break;
            }
            let value = row_data[col_idx].var_char_value().unwrap_or("");
            let col_type_str = col_infos[col_idx].r#type().to_string();
            let ddb_type = map_type(col_type_str).unwrap_or(TypeId::Varchar);
            unsafe { populate_column(value, ddb_type, chunk, row_idx, col_idx) };
        }
    }

    unsafe { duckdb_data_chunk_set_size(chunk, result_size as idx_t) };

    Ok(())
}

fn status(resp: &GetQueryExecutionOutput) -> Option<QueryExecutionState> {
    resp.query_execution()
        .and_then(|qe| qe.status())
        .and_then(|s| s.state())
        .cloned()
}

fn print_query_stats(resp: &GetQueryExecutionOutput) {
    let stats = resp.query_execution().and_then(|qe| qe.statistics());
    let Some(s) = stats else { return };

    if let Some(queue_ms) = s.query_queue_time_in_millis() {
        println!("Time in queue: {} ms", queue_ms);
    }
    if let Some(run_ms) = s.engine_execution_time_in_millis() {
        println!("Run time: {} ms", run_ms);
    }
    if let Some(bytes) = s.data_scanned_in_bytes() {
        println!("Data scanned: {}", format_bytes(bytes));
    }
}

fn format_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} bytes", bytes)
    }
}

/// # Safety
#[no_mangle]
unsafe extern "C" fn read_athena_bind(bind_info: duckdb_bind_info) {
    unsafe {
        let bi = BindInfo::new(bind_info);
        if bi.parameter_count() < 2 {
            bi.set_error("athena_scan requires at least 2 parameters: tablename, output_location");
            return;
        }

        let tablename = match bi.get_parameter_value(0).as_str() {
            Ok(s) => s,
            Err(e) => {
                bi.set_error(&e.to_string());
                return;
            }
        };
        let output_location = match bi.get_parameter_value(1).as_str() {
            Ok(s) => s,
            Err(e) => {
                bi.set_error(&e.to_string());
                return;
            }
        };
        let maxrows_val = bi.get_named_parameter_value("maxrows");
        let maxrows = if maxrows_val.is_null() { 0 } else { maxrows_val.as_i32() };
        let database = {
            let db_val = bi.get_named_parameter_value("database").as_str();
            match db_val {
                Ok(s) if !s.trim().is_empty() => s.trim().to_owned(),
                _ => "default".to_owned(),
            }
        };

        let config = crate::RUNTIME
            .block_on(aws_config::defaults(BehaviorVersion::latest()).load());
        let client = GlueClient::new(&config);

        let table_result = crate::RUNTIME.block_on(
            client
                .get_table()
                .database_name(database.clone())
                .name(tablename.clone())
                .send(),
        );

        match table_result {
            Ok(resp) => {
                if let Some(table) = resp.table() {
                    if let Some(sd) = table.storage_descriptor() {
                        for column in sd.columns() {
                            let type_str = column.r#type().unwrap_or("varchar").to_string();
                            let type_id = map_type(type_str).unwrap_or(TypeId::Varchar);
                            bi.add_result_column(column.name(), type_id);
                        }
                    }
                    // Partition columns come after data columns in Athena's SELECT * results.
                    // Registering them here keeps the DuckDB chunk column count in sync.
                    for column in table.partition_keys() {
                        let type_str = column.r#type().unwrap_or("varchar").to_string();
                        let type_id = map_type(type_str).unwrap_or(TypeId::Varchar);
                        bi.add_result_column(column.name(), type_id);
                    }
                }
            }
            Err(err) => {
                bi.set_error(&err.into_service_error().to_string());
                return;
            }
        }

        let limit = if maxrows > 0 { maxrows } else { DEFAULT_LIMIT };
        FfiBindData::<ScanBindData>::set(bind_info, ScanBindData::new(&tablename, &database, &output_location, limit));
    }
}

/// # Safety
#[no_mangle]
unsafe extern "C" fn read_athena_init(info: duckdb_init_info) {
    unsafe {
        let bind_data = match FfiBindData::<ScanBindData>::get_from_init(info) {
            Some(d) => d,
            None => return,
        };

        let tablename = bind_data.tablename.clone();
        let database = bind_data.database.clone();
        let output_location = bind_data.output_location.clone();
        let maxrows = bind_data.limit;

        let config = crate::RUNTIME
            .block_on(aws_config::defaults(BehaviorVersion::latest()).load());
        let client = AthenaClient::new(&config);

        let result_config = ResultConfiguration::builder()
            .output_location(output_location)
            .build();

        let qualified_table = format!("\"{}\".\"{}\"", database.replace('"', ""), tablename.replace('"', ""));
        let mut query = format!("SELECT * FROM {}", qualified_table);
        if maxrows > 0 {
            query = format!("{} LIMIT {}", query, maxrows);
        }

        let start_resp = crate::RUNTIME.block_on(
            client
                .start_query_execution()
                .query_string(query)
                .result_configuration(result_config)
                .work_group("primary")
                .send(),
        );

        let query_execution_id = match start_resp {
            Ok(r) => r.query_execution_id().unwrap_or_default().to_string(),
            Err(e) => {
                let msg = CString::new(e.to_string()).unwrap_or_default();
                libduckdb_sys::duckdb_init_set_error(info, msg.as_ptr());
                return;
            }
        };

        println!("Running Athena query, execution id: {}", &query_execution_id);

        loop {
            let get_resp = crate::RUNTIME.block_on(
                client
                    .get_query_execution()
                    .query_execution_id(query_execution_id.clone())
                    .send(),
            );

            let resp = match get_resp {
                Ok(r) => r,
                Err(e) => {
                    let msg = CString::new(e.to_string()).unwrap_or_default();
                    libduckdb_sys::duckdb_init_set_error(info, msg.as_ptr());
                    return;
                }
            };

            let state = match status(&resp) {
                Some(s) => s,
                None => {
                    let msg = CString::new("Could not get query state").unwrap_or_default();
                    libduckdb_sys::duckdb_init_set_error(info, msg.as_ptr());
                    return;
                }
            };

            match state {
                Queued | Running => {
                    thread::sleep(Duration::from_secs(5));
                    println!("State: {:?}, sleeping 5 secs...", state);
                }
                Cancelled | Failed => {
                    let msg = format!("Query {:?}: {}", state, query_execution_id);
                    let c_msg = CString::new(msg).unwrap_or_default();
                    libduckdb_sys::duckdb_init_set_error(info, c_msg.as_ptr());
                    return;
                }
                _ => {
                    print_query_stats(&resp);

                    // Collect all pages from the paginator
                    let mut pages: Vec<GetQueryResultsOutput> = Vec::new();
                    let mut paginator = client
                        .get_query_results()
                        .query_execution_id(query_execution_id.clone())
                        .into_paginator()
                        .send();

                    loop {
                        let next = crate::RUNTIME.block_on(paginator.next());
                        match next {
                            Some(Ok(page)) => pages.push(page),
                            Some(Err(e)) => {
                                let msg = CString::new(e.to_string()).unwrap_or_default();
                                libduckdb_sys::duckdb_init_set_error(info, msg.as_ptr());
                                return;
                            }
                            None => break,
                        }
                    }

                    FfiInitData::<ScanInitData>::set(info, ScanInitData::new(pages));
                    break;
                }
            }
        }
    }
}

pub fn build_table_function_def() -> TableFunctionBuilder {
    TableFunctionBuilder::new("athena_scan")
        .param(TypeId::Varchar)
        .param(TypeId::Varchar)
        .named_param("maxrows", TypeId::Integer)
        .named_param("database", TypeId::Varchar)
        .bind(read_athena_bind)
        .init(read_athena_init)
        .scan(read_athena)
}

