mod client;
mod error;
mod handlers;
mod models;
mod rpc;
mod utils;

use client::Session;
use error::PluginResult;
use handlers::{crud, ddl, metadata, query};
use models::{ColumnDefinition, ConnectionParams, PluginSettings};
use rpc::{RpcError, RpcResult};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead};
use utils::binding::DEFAULT_MAX_BLOB_SIZE;

/// The arguments of one JSON-RPC request, with the plugin settings needed to
/// act on them.
struct Request<'a> {
    params: &'a Value,
    settings: &'a PluginSettings,
}

impl Request<'_> {
    /// First non-empty string among `keys`. Tabularis sends `view_name`,
    /// `routine_name` and `fk_name`; its plugin guide documents `view`,
    /// `routine` and `constraint_name`. Both spellings are accepted.
    fn text(&self, keys: &[&str]) -> Option<&str> {
        keys.iter().find_map(|key| {
            self.params
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
    }

    fn required(&self, keys: &[&str]) -> Result<&str, RpcError> {
        self.text(keys)
            .ok_or_else(|| RpcError::invalid_params(format!("Missing parameter '{}'", keys[0])))
    }

    fn connection(&self) -> Result<ConnectionParams, RpcError> {
        let raw = self.params.get("params").unwrap_or(&Value::Null);
        let parsed: ConnectionParams = serde_json::from_value(raw.clone())
            .map_err(|error| RpcError::invalid_params(format!("Invalid connection: {error}")))?;
        if parsed.driver != "db2" {
            return Err(RpcError::invalid_params(format!(
                "Unsupported driver '{}'",
                parsed.driver
            )));
        }
        Ok(parsed)
    }

    /// The schema a request works in: the one Tabularis selected, else the
    /// plugin's "Default Schema" setting, else the user's own schema (Db2
    /// names it after the authorization ID, in upper case).
    fn schema(&self, connection: &ConnectionParams) -> String {
        self.text(&["schema"])
            .map(ToString::to_string)
            .or_else(|| {
                self.settings
                    .current_schema
                    .clone()
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| {
                connection
                    .username
                    .as_deref()
                    .unwrap_or_default()
                    .to_uppercase()
            })
    }

    /// Opens the connection for this request, without selecting a schema.
    fn session(&self) -> Result<Session, RpcError> {
        let connection = self.connection()?;
        Session::open(&connection, self.settings, None).map_err(RpcError::database)
    }

    /// Opens the connection for this request with its schema as the current
    /// schema, and returns that schema for the catalog queries.
    fn schema_session(&self) -> Result<(Session, String), RpcError> {
        let connection = self.connection()?;
        let schema = self.schema(&connection);
        let session =
            Session::open(&connection, self.settings, Some(&schema)).map_err(RpcError::database)?;
        Ok((session, schema))
    }

    fn column(&self, key: &str) -> Result<ColumnDefinition, RpcError> {
        serde_json::from_value(self.params.get(key).cloned().unwrap_or(Value::Null))
            .map_err(|error| RpcError::invalid_params(format!("Invalid '{key}': {error}")))
    }

    fn columns(&self, key: &str) -> Result<Vec<ColumnDefinition>, RpcError> {
        serde_json::from_value(self.params.get(key).cloned().unwrap_or_else(|| json!([])))
            .map_err(|error| RpcError::invalid_params(format!("Invalid '{key}': {error}")))
    }

    fn strings(&self, key: &str) -> Option<Vec<String>> {
        self.params
            .get(key)
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect()
            })
    }

    /// A JSON object argument (`data`, `pk_map`); a missing one is empty.
    fn map(&self, key: &str) -> Result<HashMap<String, Value>, RpcError> {
        match self.params.get(key) {
            None | Some(Value::Null) => Ok(HashMap::new()),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| RpcError::invalid_params(format!("Invalid '{key}': {error}"))),
        }
    }

    /// The key values that identify a row: `pk_map`, or the single
    /// `pk_col` / `pk_val` pair Tabularis sent before 0.13.4.
    fn row_keys(&self) -> Result<HashMap<String, Value>, RpcError> {
        let mut keys = self.map("pk_map")?;
        if keys.is_empty() {
            if let Some(column) = self.text(&["pk_col"]) {
                let value = self.params.get("pk_val").cloned().unwrap_or(Value::Null);
                keys.insert(column.to_string(), value);
            }
        }
        Ok(keys)
    }

    /// Largest BLOB the host allows in a write, from its settings.
    fn max_blob_size(&self) -> u64 {
        self.params
            .get("max_blob_size")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_MAX_BLOB_SIZE)
    }

    fn u32(&self, keys: &[&str]) -> Option<u32> {
        keys.iter().find_map(|key| {
            self.params
                .get(key)
                .and_then(Value::as_u64)
                .map(|value| value.min(u32::MAX as u64) as u32)
        })
    }
}

/// Serializes a handler's outcome into a JSON-RPC result or database error.
fn reply<T: Serialize>(result: PluginResult<T>) -> RpcResult {
    let value = result.map_err(RpcError::database)?;
    serde_json::to_value(value).map_err(|error| RpcError::database(error.to_string()))
}

fn dispatch(method: &str, request: &Request) -> RpcResult {
    match method {
        // -- Connection ------------------------------------------------------
        "test_connection" => {
            let session = request.session()?;
            reply(query::test_connection(&session).map(|()| json!({ "success": true })))
        }
        "ping" => {
            let session = request.session()?;
            reply(query::test_connection(&session))
        }

        // -- Schema discovery ------------------------------------------------
        "get_databases" => reply(metadata::get_databases(&request.session()?)),
        "get_schemas" => reply(metadata::get_schemas(&request.session()?)),
        "get_tables" => {
            let (session, schema) = request.schema_session()?;
            reply(metadata::get_tables(&session, &schema))
        }
        "get_columns" => {
            let (session, schema) = request.schema_session()?;
            let table = request.required(&["table", "table_name"])?;
            reply(metadata::get_columns(&session, &schema, table))
        }
        "get_foreign_keys" => {
            let (session, schema) = request.schema_session()?;
            let table = request.required(&["table", "table_name"])?;
            reply(metadata::get_foreign_keys(&session, &schema, table))
        }
        "get_indexes" => {
            let (session, schema) = request.schema_session()?;
            let table = request.required(&["table", "table_name"])?;
            reply(metadata::get_indexes(&session, &schema, table))
        }
        "get_all_columns_batch" => {
            let (session, schema) = request.schema_session()?;
            let tables = request.strings("tables");
            reply(metadata::get_all_columns_batch(
                &session,
                &schema,
                tables.as_deref(),
            ))
        }
        "get_all_foreign_keys_batch" => {
            let (session, schema) = request.schema_session()?;
            let tables = request.strings("tables");
            reply(metadata::get_all_foreign_keys_batch(
                &session,
                &schema,
                tables.as_deref(),
            ))
        }
        "get_schema_snapshot" => {
            let (session, schema) = request.schema_session()?;
            reply(metadata::get_schema_snapshot(&session, &schema))
        }

        // -- Views -----------------------------------------------------------
        "get_views" => {
            let (session, schema) = request.schema_session()?;
            reply(metadata::get_views(&session, &schema))
        }
        "get_view_definition" => {
            let (session, schema) = request.schema_session()?;
            let view = request.required(&["view_name", "view", "name"])?;
            reply(metadata::get_view_definition(&session, &schema, view))
        }
        "get_view_columns" => {
            let (session, schema) = request.schema_session()?;
            let view = request.required(&["view_name", "view", "name"])?;
            reply(metadata::get_columns(&session, &schema, view))
        }

        "create_view" => {
            let (session, schema) = request.schema_session()?;
            let view = request.required(&["view_name", "view", "name"])?;
            let definition = request.required(&["definition"])?;
            reply(metadata::create_view(&session, &schema, view, definition).map(|()| Value::Null))
        }
        "alter_view" => {
            let (session, schema) = request.schema_session()?;
            let view = request.required(&["view_name", "view", "name"])?;
            let definition = request.required(&["definition"])?;
            reply(metadata::alter_view(&session, &schema, view, definition).map(|()| Value::Null))
        }
        "drop_view" => {
            let (session, schema) = request.schema_session()?;
            let view = request.required(&["view_name", "view", "name"])?;
            reply(metadata::drop_view(&session, &schema, view).map(|()| Value::Null))
        }

        // -- Routines --------------------------------------------------------
        "get_routines" => {
            let (session, schema) = request.schema_session()?;
            reply(metadata::get_routines(&session, &schema))
        }
        "get_routine_parameters" => {
            let (session, schema) = request.schema_session()?;
            let routine = request.required(&["routine_name", "routine"])?;
            reply(metadata::get_routine_parameters(&session, &schema, routine))
        }
        "get_routine_definition" => {
            let (session, schema) = request.schema_session()?;
            let routine = request.required(&["routine_name", "routine"])?;
            reply(metadata::get_routine_definition(&session, &schema, routine))
        }

        // -- Queries ---------------------------------------------------------
        "execute_query" => {
            let (session, _) = request.schema_session()?;
            reply(query::execute_query(
                &session,
                request.text(&["query"]).unwrap_or_default(),
                request.u32(&["limit", "page_size"]),
                request.u32(&["page"]).unwrap_or(1),
            ))
        }
        "execute_query_batch" => {
            let (session, _) = request.schema_session()?;
            let queries = request
                .strings("queries")
                .ok_or_else(|| RpcError::invalid_params("Missing parameter 'queries'"))?;
            Ok(json!(query::execute_query_batch(
                &session,
                &queries,
                request.u32(&["limit", "page_size"]),
                request.u32(&["page"]).unwrap_or(1),
            )))
        }
        "explain_query" => {
            let (session, _) = request.schema_session()?;
            reply(query::explain_query(
                &session,
                request.text(&["query"]).unwrap_or_default(),
                request
                    .params
                    .get("analyze")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ))
        }

        // -- Records ---------------------------------------------------------
        "insert_record" => {
            let (session, schema) = request.schema_session()?;
            reply(crud::insert_record(
                &session,
                request.required(&["table"])?,
                &request.map("data")?,
                &schema,
                request.max_blob_size(),
            ))
        }
        "fetch_blob_as_data_url" => {
            let (session, schema) = request.schema_session()?;
            reply(crud::fetch_blob_as_data_url(
                &session,
                request.required(&["table"])?,
                request.required(&["col_name"])?,
                &request.map("pk_map")?,
                &schema,
            ))
        }
        "save_blob_to_file" => {
            let (session, schema) = request.schema_session()?;
            reply(crud::save_blob_to_file(
                &session,
                request.required(&["table"])?,
                request.required(&["col_name"])?,
                &request.map("pk_map")?,
                &schema,
                request.required(&["file_path"])?,
            ))
        }
        "update_record" => {
            let (session, schema) = request.schema_session()?;
            reply(crud::update_record(
                &session,
                request.required(&["table"])?,
                &request.row_keys()?,
                request.required(&["col_name"])?,
                request.params.get("new_val").unwrap_or(&Value::Null),
                &schema,
                request.max_blob_size(),
            ))
        }
        "delete_record" => {
            let (session, schema) = request.schema_session()?;
            reply(crud::delete_record(
                &session,
                request.required(&["table"])?,
                &request.row_keys()?,
                &schema,
            ))
        }

        // -- DDL generation (pure SQL builders, no connection) ----------------
        "get_create_table_sql" => reply(ddl::get_create_table_sql(
            request.text(&["table_name", "table"]).unwrap_or_default(),
            &request.columns("columns")?,
            request.text(&["schema"]),
        )),
        "get_add_column_sql" => Ok(json!(ddl::get_add_column_sql(
            request.required(&["table"])?,
            &request.column("column")?,
            request.text(&["schema"]),
        ))),
        "get_alter_column_sql" => Ok(json!(ddl::get_alter_column_sql(
            request.required(&["table"])?,
            &request.column("old_column")?,
            &request.column("new_column")?,
            request.text(&["schema"]),
        ))),
        "get_create_index_sql" => reply(ddl::get_create_index_sql(
            request.required(&["table"])?,
            request.text(&["index_name"]).unwrap_or_default(),
            &request.strings("columns").unwrap_or_default(),
            request
                .params
                .get("is_unique")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            request.text(&["schema"]),
        )),
        "get_create_foreign_key_sql" => reply(ddl::get_create_foreign_key_sql(
            request.required(&["table"])?,
            request.text(&["fk_name"]).unwrap_or_default(),
            request.required(&["column"])?,
            request.required(&["ref_table"])?,
            request.required(&["ref_column"])?,
            request.text(&["on_delete"]),
            request.text(&["on_update"]),
            request.text(&["schema"]),
        )),
        "get_table_ddl" => {
            let (session, schema) = request.schema_session()?;
            reply(metadata::get_table_ddl(
                &session,
                &schema,
                request.required(&["table"])?,
            ))
        }

        // -- DDL execution ---------------------------------------------------
        "drop_index" => {
            let (session, schema) = request.schema_session()?;
            reply(
                metadata::drop_index(
                    &session,
                    &schema,
                    request.required(&["table"])?,
                    request.required(&["index_name"])?,
                )
                .map(|()| Value::Null),
            )
        }
        "drop_foreign_key" => {
            let (session, schema) = request.schema_session()?;
            reply(
                metadata::drop_foreign_key(
                    &session,
                    &schema,
                    request.required(&["table"])?,
                    request.required(&["fk_name", "constraint_name"])?,
                )
                .map(|()| Value::Null),
            )
        }

        _ => Err(RpcError::method_not_found(method)),
    }
}

/// Makes the Db2 CLI driver exchange character data as UTF-8 (code page
/// 1208) whatever the process locale is.
///
/// The driver derives its application code page from the locale. Under the
/// `C` locale — what a container or a service usually runs with — that is a
/// single-byte code page, and every character outside it (accented letters,
/// CJK) reaches the plugin as a substitution character. The plugin always
/// reads and writes UTF-8, so the code page is pinned unless the user set
/// `DB2CODEPAGE` themselves. Must run before the first ODBC call.
fn pin_utf8_code_page() {
    if std::env::var_os("DB2CODEPAGE").is_none() {
        std::env::set_var("DB2CODEPAGE", "1208");
    }
}

fn main() {
    pin_utf8_code_page();

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut settings = PluginSettings::default();

    // Tabularis closes stdin when the session ends; reaching EOF ends the
    // loop and the process with it.
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            Ok(_) => continue,
            Err(error) => {
                eprintln!("Failed to read stdin: {error}");
                break;
            }
        };

        let request: Value = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                eprintln!("Invalid JSON-RPC payload: {error}");
                continue;
            }
        };

        let method = request.get("method").and_then(Value::as_str);
        // A request without an id is a notification (for example `cancel`):
        // it must not be answered, or the reply would land on stdout as an
        // unexpected line.
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let Some(method) = method else {
            let _ = rpc::send_error(&mut stdout, id, -32600, "Method not specified");
            continue;
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

        let outcome = if method == "initialize" {
            settings = serde_json::from_value(
                params.get("settings").cloned().unwrap_or_else(|| json!({})),
            )
            .unwrap_or_default();
            Ok(Value::Null)
        } else {
            dispatch(
                method,
                &Request {
                    params: &params,
                    settings: &settings,
                },
            )
        };

        let written = match outcome {
            Ok(result) => rpc::send_success(&mut stdout, id, result),
            Err(error) => rpc::send_error(&mut stdout, id, error.code, &error.message),
        };
        if let Err(error) = written {
            eprintln!("Failed to write JSON-RPC response: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{dispatch, Request};
    use crate::models::PluginSettings;
    use serde_json::json;

    fn schema_of(params: serde_json::Value, settings: &PluginSettings) -> String {
        let request = Request {
            params: &params,
            settings,
        };
        let connection = request.connection().expect("valid connection");
        request.schema(&connection)
    }

    fn connection() -> serde_json::Value {
        json!({ "driver": "db2", "host": "h", "port": 50000, "username": "db2inst1",
                "password": "x", "database": "SAMPLE" })
    }

    #[test]
    fn schema_falls_back_to_the_setting_then_to_the_users_own_schema() {
        let defaults = PluginSettings::default();
        let with_setting = PluginSettings {
            current_schema: Some("APP".to_string()),
            ..PluginSettings::default()
        };

        // The schema Tabularis selected wins, exactly as written.
        let selected = json!({ "params": connection(), "schema": "Mixed Case" });
        assert_eq!(schema_of(selected.clone(), &with_setting), "Mixed Case");

        // Without one (null or empty), the "Default Schema" setting applies.
        let unselected = json!({ "params": connection(), "schema": null });
        assert_eq!(schema_of(unselected.clone(), &with_setting), "APP");
        let empty = json!({ "params": connection(), "schema": "" });
        assert_eq!(schema_of(empty, &with_setting), "APP");

        // Without either, Db2's implicit schema: the authorization ID in
        // upper case, which is how the catalog stores it.
        assert_eq!(schema_of(unselected, &defaults), "DB2INST1");
    }

    #[test]
    fn rejects_bad_requests_before_connecting() {
        let settings = PluginSettings::default();
        let run = |method: &str, params: serde_json::Value| {
            dispatch(
                method,
                &Request {
                    params: &params,
                    settings: &settings,
                },
            )
        };

        let error = run("no_such_method", json!({})).unwrap_err();
        assert_eq!(error.code, -32601);
        assert_eq!(error.message, "Method not found: no_such_method");

        let error = run("get_tables", json!({})).unwrap_err();
        assert_eq!(error.code, -32602);

        let mut other_driver = connection();
        other_driver["driver"] = json!("postgres");
        let error = run("get_tables", json!({ "params": other_driver })).unwrap_err();
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, "Unsupported driver 'postgres'");
    }
}
