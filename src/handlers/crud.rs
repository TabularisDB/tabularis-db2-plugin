use crate::client::execute_statement;
use crate::error::PluginResult;
use crate::models::{ConnectionParams, PluginSettings};
use crate::utils::identifiers::{quote_identifier, quote_qualified_name};
use crate::utils::values::json_to_sql_literal;
use serde_json::Value;
use std::collections::HashMap;

pub fn insert_record(
    params: &ConnectionParams,
    settings: &PluginSettings,
    table: &str,
    data: &HashMap<String, Value>,
    schema: Option<&str>,
) -> PluginResult<u64> {
    let mut entries = data.iter().collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let columns = entries
        .iter()
        .map(|(name, _)| quote_identifier(name))
        .collect::<Vec<_>>()
        .join(", ");
    let values = entries
        .iter()
        .map(|(_, value)| json_to_sql_literal(value))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_qualified_name(schema, table),
        columns,
        values
    );
    execute_statement(params, settings, schema, &sql)
}

#[allow(clippy::too_many_arguments)]
pub fn update_record(
    params: &ConnectionParams,
    settings: &PluginSettings,
    table: &str,
    pk_col: &str,
    pk_val: &Value,
    col_name: &str,
    new_val: &Value,
    schema: Option<&str>,
) -> PluginResult<u64> {
    let sql = format!(
        "UPDATE {} SET {} = {} WHERE {} = {}",
        quote_qualified_name(schema, table),
        quote_identifier(col_name),
        json_to_sql_literal(new_val),
        quote_identifier(pk_col),
        json_to_sql_literal(pk_val)
    );
    execute_statement(params, settings, schema, &sql)
}

pub fn delete_record(
    params: &ConnectionParams,
    settings: &PluginSettings,
    table: &str,
    pk_col: &str,
    pk_val: &Value,
    schema: Option<&str>,
) -> PluginResult<u64> {
    let sql = format!(
        "DELETE FROM {} WHERE {} = {}",
        quote_qualified_name(schema, table),
        quote_identifier(pk_col),
        json_to_sql_literal(pk_val)
    );
    execute_statement(params, settings, schema, &sql)
}
